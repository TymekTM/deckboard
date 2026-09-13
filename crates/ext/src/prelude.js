// Deckboard extension shims. Injected into every extension's JS context
// before its entry point loads. `__EXT_ROOT` and `__EXT_PACKAGE` are
// interpolated by the host.
//
// Design: all state flows through JS globals that the Rust side drains
// after each eval (`__pending_set_values`, `__new_intervals`,
// `__cleared_intervals`), so no native function needs captured state.
"use strict";

// placeholders are replaced by the host with quoted JSON string literals
// ------------------------------------------------------------ console
function __console_fmt(args) {
    var parts = [];
    for (var i = 0; i < args.length; i++) {
        var a = args[i];
        try { parts.push(typeof a === "string" ? a : JSON.stringify(a)); }
        catch (e) { parts.push(String(a)); }
    }
    return parts.join(" ");
}
var console = {
    log: function () { __host_log("info", __console_fmt(arguments)); },
    info: function () { __host_log("info", __console_fmt(arguments)); },
    warn: function () { __host_log("warn", __console_fmt(arguments)); },
    error: function () { __host_log("error", __console_fmt(arguments)); },
    debug: function () { __host_log("debug", __console_fmt(arguments)); },
};

// Boa lacks the Annex B substr that the bundled moment.js uses
if (!String.prototype.substr) {
    String.prototype.substr = function (start, length) {
        var n = this.length;
        if (start < 0) start = Math.max(0, n + start);
        if (length === undefined) length = n - start;
        return this.slice(start, start + Math.max(0, length));
    };
}

var __EXT_ROOT = __EXT_ROOT__;
var __EXT_PACKAGE = __EXT_PACKAGE__;

// ---------------------------------------------------------------- timers
var __timer_seq = 0;
var __timers = {};            // id -> { cb, ms, next (managed host-side) }
var __new_intervals = [];     // drained by Rust: { id, ms }
var __cleared_intervals = []; // drained by Rust: id

function __run_timer(id) {
    var t = __timers[id];
    if (t && typeof t.cb === "function") {
        try {
            t.cb();
        } catch (e) {
            __host_log("error", __EXT_PACKAGE + " timer: " + (e && e.message || e) + (e && e.stack ? " @ " + e.stack : ""));
        }
    }
}

function setInterval(cb, ms) {
    var id = ++__timer_seq;
    __timers[id] = { cb: cb, ms: ms };
    __new_intervals.push({ id: id, ms: ms });
    return id;
}

function clearInterval(id) {
    delete __timers[id];
    __cleared_intervals.push(id);
}

function setTimeout(cb, ms) {
    // emulated over an interval that clears itself after one tick
    var wrapped = function () {
        clearInterval(id);
        cb();
    };
    var id = setInterval(wrapped, ms === undefined ? 0 : ms);
    return id;
}

function clearTimeout(id) {
    clearInterval(id);
}

// ------------------------------------------------------- set_value queue
var __pending_set_values = []; // drained by Rust: JSON objects

function __flush_set_values() {
    return __pending_set_values;
}

// --------------------------------------------------------- native bridge
// provided by the host as plain functions: see host.rs
// __host_read_file(path) -> string|null
// __host_file_exists(path) -> bool
// __host_list_dir(path) -> [names] | null
// __host_write_file(path, text) -> bool
// __host_shell_exec(cmd) -> {stdout, stderr, error}
// __host_spawn(cmd) -> bool
// __host_open(target) -> bool
// __host_http(spec) -> {status, body, error}
// __host_dialog_error(title, msg)
// __host_log(level, msg)

// ------------------------------------------------------------ fs (subset)
var __path = (function () {
    function join() {
        var parts = [];
        for (var i = 0; i < arguments.length; i++) {
            var p = String(arguments[i]);
            if (p !== "") parts.push(p);
        }
        var joined = parts.join("/");
        return joined.replace(/\\/g, "/").replace(/\/{2,}/g, "/");
    }
    function dirname(p) {
        p = String(p).replace(/\\/g, "/");
        var i = p.lastIndexOf("/");
        return i <= 0 ? "." : p.slice(0, i);
    }
    function basename(p) {
        p = String(p).replace(/\\/g, "/");
        var i = p.lastIndexOf("/");
        return i === -1 ? p : p.slice(i + 1);
    }
    function extname(p) {
        var b = basename(p);
        var i = b.lastIndexOf(".");
        return i <= 0 ? "" : b.slice(i);
    }
    function resolve() {
        var p = join.apply(null, arguments);
        if (/^[a-zA-Z]:/.test(p) || p.charAt(0) === "/") return p;
        return join(__EXT_ROOT, p);
    }
    function normalize(p) {
        var segs = String(p).replace(/\\/g, "/").split("/");
        var out = [];
        for (var i = 0; i < segs.length; i++) {
            var s = segs[i];
            if (s === "." || s === "") continue;
            if (s === ".." && out.length && out[out.length - 1] !== "..") out.pop();
            else out.push(s);
        }
        return out.join("/");
    }
    return {
        join: join,
        dirname: dirname,
        basename: basename,
        extname: extname,
        resolve: resolve,
        normalize: normalize,
        sep: "/",
        delimiter: ";",
    };
})();

var __fs = {
    readFileSync: function (p, enc) {
        var data = __host_read_file(String(p));
        if (data === null) throw new Error("ENOENT: no such file or directory, open '" + p + "'");
        if (enc === "base64") return __host_read_file_base64(String(p));
        return data;
    },
    existsSync: function (p) {
        return __host_file_exists(String(p));
    },
    readdirSync: function (p) {
        var list = __host_list_dir(String(p));
        if (list === null) throw new Error("ENOENT: no such directory, open '" + p + "'");
        return list;
    },
    writeFileSync: function (p, data) {
        return __host_write_file(String(p), String(data));
    },
    statSync: function (p) {
        return { isFile: function () { return __host_file_exists(String(p)); }, isDirectory: function () { return false; } };
    },
    createReadStream: function () {
        throw new Error("fs.createReadStream is not supported by deckboard-ext");
    },
    closeSync: function () {},
    close: function (fd, cb) { if (typeof cb === "function") cb(null); },
    openSync: function () { throw new Error("fs.open is not supported by deckboard-ext"); },
    constants: {},
    F_OK: 0, R_OK: 4, W_OK: 2, X_OK: 1,
    accessSync: function (p) {
        if (!__host_file_exists(String(p))) throw new Error("EACCES: permission denied, access '" + p + "'");
    },
    appendFileSync: function (p, data) {
        var cur = __host_file_exists(String(p)) ? __host_read_file(String(p)) || "" : "";
        __host_write_file(String(p), cur + String(data));
    },
};

// fs-extra enumerates every callback fs method and wraps it with
// universalify, so each name must exist as a function or it throws
function __fs_not_supported(name) {
    return function () {
        var cb = arguments[arguments.length - 1];
        var err = new Error("fs." + name + " is not supported by deckboard-ext");
        if (typeof cb === "function") cb(err);
        else throw err;
    };
}
["access", "appendFile", "chmod", "chown", "copyFile", "fchmod", "fchown",
 "fdatasync", "fstat", "fsync", "ftruncate", "futimes", "lchown", "lchmod",
 "link", "lstat", "mkdir", "mkdtemp", "open", "readlink", "realpath",
 "rename", "rmdir", "symlink", "truncate", "unlink", "utimes", "writeFile",
].forEach(function (name) { __fs[name] = __fs_not_supported(name); });
__fs.readFile = function (p, cb) {
    try { cb(null, __fs.readFileSync(p)); } catch (e) { cb(e); }
};
__fs.readdir = function (p, cb) {
    try { cb(null, __fs.readdirSync(p)); } catch (e) { cb(e); }
};
__fs.stat = function (p, cb) {
    try { cb(null, __fs.statSync(p)); } catch (e) { cb(e); }
};
__fs.exists = function (p, cb) { cb(__fs.existsSync(p)); };
__fs.promises = {};

var __fs_extra = {
    readJsonSync: function (p) {
        return JSON.parse(__fs.readFileSync(p));
    },
    writeJsonSync: function (p, obj) {
        __fs.writeFileSync(p, JSON.stringify(obj, null, 2));
    },
    readFile: function (p, cb) {
        try {
            cb(null, __fs.readFileSync(p));
        } catch (e) {
            cb(e);
        }
    },
    readJson: function (p, cb) {
        try {
            cb(null, __fs_extra.readJsonSync(p));
        } catch (e) {
            cb(e);
        }
    },
    existsSync: __fs.existsSync,
    pathExistsSync: __fs.existsSync,
    writeFile: function (p, data, cb) {
        __fs.writeFileSync(p, data);
        if (cb) cb(null);
    },
};

// ------------------------------------------------------------------ path module re-export
var __path_mod = __path;

// --------------------------------------------------------------------- os
var __os = {
    platform: function () { return "win32"; },
    arch: function () { return "x64"; },
    type: function () { return "Windows_NT"; },
    release: function () { return "10.0.0"; },
    hostname: function () { return __host_hostname(); },
    homedir: function () { return __host_home_dir(); },
    tmpdir: function () { return __host_tmp_dir(); },
    EOL: "\r\n",
    loadavg: function () { return [0, 0, 0]; },
    uptime: function () { return 0; },
    cpus: function () {
        // one synthetic core: si computes load percentages from cpu times
        return [{
            model: "CPU",
            speed: 0,
            times: { user: 0, nice: 0, sys: 0, idle: 100000, irq: 0 },
        }];
    },
    totalmem: function () { return 0; },
    freemem: function () { return 0; },
};

// ------------------------------------------------------------------ events
function __EventEmitter() {
    if (!(this instanceof __EventEmitter)) return new __EventEmitter();
    this._handlers = {};
}
__EventEmitter.prototype.on = function (ev, cb) {
    (this._handlers[ev] = this._handlers[ev] || []).push(cb);
    return this;
};
__EventEmitter.prototype.once = function (ev, cb) {
    var self = this;
    var wrapped = function () {
        self.off(ev, wrapped);
        cb.apply(self, arguments);
    };
    return self.on(ev, wrapped);
};
__EventEmitter.prototype.off = function (ev, cb) {
    var list = this._handlers[ev] || [];
    var i = list.indexOf(cb);
    if (i >= 0) list.splice(i, 1);
    return this;
};
__EventEmitter.prototype.removeListener = __EventEmitter.prototype.off;
__EventEmitter.prototype.emit = function (ev) {
    var list = (this._handlers[ev] || []).slice();
    var args = Array.prototype.slice.call(arguments, 1);
    for (var i = 0; i < list.length; i++) list[i].apply(this, args);
    return list.length > 0;
};
// require('events') is the constructor itself, but keep named exports working
__EventEmitter.EventEmitter = __EventEmitter;
__EventEmitter.default = __EventEmitter;
__EventEmitter.Event = __EventEmitter;

// ------------------------------------------------------------------- util
var __util = {
    _extend: function (target) {
        for (var i = 1; i < arguments.length; i++) {
            var src = arguments[i] || {};
            for (var k in src) if (Object.prototype.hasOwnProperty.call(src, k)) target[k] = src[k];
        }
        return target;
    },
    deprecate: function (fn, msg) {
        return function () { return fn.apply(this, arguments); };
    },
    format: function (f) {
        var args = Array.prototype.slice.call(arguments, 1);
        var used = 0;
        var out = String(f).replace(/%[sdifjoO%]/g, function (m) {
            if (m === "%%") return "%";
            var v = args[used++];
            if (v === undefined) return "";
            if (m === "%j") { try { return JSON.stringify(v); } catch (e) { return ""; } }
            return String(v);
        });
        for (var i = used; i < args.length; i++) out += " " + String(args[i]);
        return out;
    },
    promisify: function (fn) {
        return function () {
            var args = Array.prototype.slice.call(arguments);
            return new Promise(function (resolve, reject) {
                fn.apply(null, args.concat(function (err, value) {
                    if (err) reject(err);
                    else resolve(value);
                }));
            });
        };
    },
    inspect: function (v) {
        try {
            return JSON.stringify(v);
        } catch (e) {
            return String(v);
        }
    },
    inherits: function (ctor, superCtor) {
        ctor.prototype = Object.create(superCtor.prototype, {
            constructor: { value: ctor, enumerable: false },
        });
    },
    callbackify: function (fn) {
        return function () {
            var args = Array.prototype.slice.call(arguments);
            var cb = args.pop();
            try {
                var result = fn.apply(null, args);
                if (result && typeof result.then === "function") {
                    result.then(function (v) { cb(null, v); }, function (e) { cb(e); });
                } else {
                    cb(null, result);
                }
            } catch (e) {
                cb(e);
            }
        };
    },
};

// ---------------------------------------------------------- child_process
var __child_process = {
    exec: function (cmd, opts, cb) {
        if (typeof opts === "function") {
            cb = opts;
            opts = {};
        }
        var res = __host_shell_exec(String(cmd));
        var child = {
            killed: true,
            kill: function () {},
            on: function () { return child; },
            stdout: { on: function () {} },
            stderr: { on: function () {} },
        };
        if (typeof cb === "function") {
            cb(res.error ? new Error(res.error) : null, res.stdout || "", res.stderr || "");
        }
        return child;
    },
    execSync: function (cmd) {
        var res = __host_shell_exec(String(cmd));
        if (res.error) throw new Error(res.error);
        return res.stdout || "";
    },
    execFile: function (file, args, opts, cb) {
        if (typeof opts === "function") {
            cb = opts;
            opts = {};
        }
        var full = file;
        if (Array.isArray(args)) full = file + " " + args.join(" ");
        return __child_process.exec(full, cb);
    },
    // spawn runs the command synchronously under the hood but delivers
    // stdout/stderr/close through callbacks like real node
    spawn: function (file, args) {
        var full = file + " " + (Array.isArray(args) ? args.join(" ") : "");
        var handlers = {};
        var child = {
            killed: false,
            pid: 0,
            kill: function () { child.killed = true; return child; },
            on: function (ev, cb) { handlers[ev] = cb; return child; },
            stdout: { on: function (ev, cb) { handlers["out_" + ev] = cb; return child.stdout; } },
            stderr: { on: function (ev, cb) { handlers["err_" + ev] = cb; return child.stderr; } },
        };
        setTimeout(function () {
            if (child.killed) return;
            var res = {};
            try { res = JSON.parse(__host_spawn_capture(full)); } catch (e) { res = { error: String(e), code: -1 }; }
            try {
                if (res.error) { if (handlers.error) handlers.error(new Error(res.error)); }
                if (res.stdout && handlers.out_data) handlers.out_data(res.stdout);
                if (res.stderr && handlers.err_data) handlers.err_data(res.stderr);
                if (handlers.close) handlers.close(res.code === undefined ? 0 : res.code);
            } catch (e) {
                __host_log("error", "spawn handler: " + (e && e.message || e) + (e && e.stack ? " @ " + e.stack : ""));
            }
        }, 0);
        return child;
    },
};

// ------------------------------------------------------------- node-fetch
function __fetch(url, opts) {
    opts = opts || {};
    var spec = {
        url: String(url),
        method: opts.method || "GET",
        headers: opts.headers || {},
        body: opts.body === undefined ? null : String(opts.body),
        timeout_ms: 15000,
    };
    return new Promise(function (resolve, reject) {
        var res = __host_http(spec);
        if (res.error) {
            reject(new Error(res.error));
            return;
        }
        var body = res.body === null || res.body === undefined ? "" : res.body;
        resolve({
            ok: res.status >= 200 && res.status < 300,
            status: res.status,
            statusText: "",
            headers: { get: function () { return null; }, raw: function () { return {}; } },
            url: String(url),
            text: function () { return Promise.resolve(body); },
            json: function () {
                return new Promise(function (res2, rej2) {
                    try {
                        res2(JSON.parse(body));
                    } catch (e) {
                        rej2(e);
                    }
                });
            },
        });
    });
}

// -------------------------------------------------------------------- opn
function __opn(target) {
    return Promise.resolve(__host_open(String(target)));
}

// ---------------------------------------------------------------- wintools
var __wintools = {
    // power-control only needs graceful logging from these; if it actually
    // calls them the command strings match its usage in the wild
    shutdown: function () { __host_shell_exec("shutdown /s /t 0"); },
    restart: function () { __host_shell_exec("shutdown /r /t 0"); },
    logoff: function () { __host_shell_exec("shutdown /l"); },
    sleep: function () { __host_shell_exec("rundll32.exe powrprof.dll,SetSuspendState 0,1,0"); },
};

// -------------------------------------------------------------- node-wmi
function __wmi_query(opts, cb) {
    // node-wmi runs `wmic path <class> [where ...] get *` and parses the list
    var cmd = "wmic path " + opts.class;
    if (opts.where) {
        var w = opts.where;
        if (typeof w === "string") {
            cmd += " where " + w;
        } else {
            var parts = [];
            for (var k in w) parts.push(k + " like '%" + w[k] + "%'");
            cmd += " where " + parts.join(" and ");
        }
    }
    cmd += " get /format:list";
    var res = __host_shell_exec(cmd);
    var rows = [];
    if (!res.error) {
        var cur = {};
        var lines = String(res.stdout || "").split(/\r?\n/);
        for (var i = 0; i < lines.length; i++) {
            var line = lines[i];
            if (line.trim() === "") {
                if (Object.keys(cur).length) rows.push(cur);
                cur = {};
                continue;
            }
            var eq = line.indexOf("=");
            if (eq > 0) cur[line.slice(0, eq)] = line.slice(eq + 1);
        }
        if (Object.keys(cur).length) rows.push(cur);
    }
    if (typeof cb === "function") cb(res.error ? new Error(res.error) : null, rows);
    return rows;
}
__wmi_query.Query = function (opts, cb) { return __wmi_query(opts, cb); };

// ------------------------------------------------------------- process/Buffer
var __process = {
    platform: "win32",
    arch: "x64",
    version: "v16.20.0",
    versions: { node: "16.20.0", v8: "9.4.146.24", uv: "1.43.0", openssl: "1.1.1" },
    env: {},
    argv: [__EXT_PACKAGE],
    cwd: function () { return __EXT_ROOT; },
    exit: function (code) { __host_log("warn", __EXT_PACKAGE + " called process.exit(" + code + ") - ignored"); },
    nextTick: function (cb) { setTimeout(cb, 0); },
    hrtime: function () {
        var ms = Date.now();
        var arr = [Math.floor(ms / 1000), (ms % 1000) * 1e6];
        arr.bigint = function () { return 0n; };
        return arr;
    },
};

var __Buffer = {
    from: function (v, enc) {
        if (typeof v === "string") {
            return { data: v, toString: function () { return v; }, length: v.length };
        }
        return { data: v, toString: function () { return String(v); }, length: v.length };
    },
    alloc: function (n) { return { data: new Uint8Array(n), length: n, toString: function () { return ""; } }; },
    isBuffer: function () { return false; },
};

// -------------------------------------------------------------- deckboard-kit
var __INPUT_METHOD = {
    INPUT_TEXT: "input:text",
    INPUT_KEY: "input:key",
    INPUT_SELECT: "input:select",
    INPUT_FILE: "input:file",
    INPUT_FOLDER: "input:input-folder",
    INPUT_FOLDER2: "input:folder",
    INPUT_COLOR: "input:color",
    INPUT_CHECKBOX: "input:checkbox",
    INPUT_TEXTAREA: "input:multilinetext",
};

var __PLATFORMS = {
    WINDOWS: "WINDOWS",
    MAC: "MAC",
    LINUX: "LINUX",
    SUN: "SUN",
    OPENBSD: "OPENBSD",
    ANDROID: "ANDROID",
    AIX: "AIX",
    windows: "WINDOWS",
    mac: "MAC",
    linux: "LINUX",
};

var __ICONS = { FONTAWESOME_SOLID: "fas", FONTAWESOME_BRAND: "fab", ICONS: "ion" };

function __Extension() {
    this.name = "";
    this.inputs = [];
    this.configs = {};
    this.platforms = ["WINDOWS"];
}
Object.defineProperty(__Extension.prototype, "selections", {
    get: function () {
        return [{ header: this.name }].concat(this.inputs);
    },
});
__Extension.prototype.execute = function () {};

var __kit_log = {
    info: function () { __host_log("info", __EXT_PACKAGE + ": " + Array.prototype.join.call(arguments, " ")); },
    warn: function () { __host_log("warn", __EXT_PACKAGE + ": " + Array.prototype.join.call(arguments, " ")); },
    error: function () { __host_log("error", __EXT_PACKAGE + ": " + Array.prototype.join.call(arguments, " ")); },
    debug: function () { __host_log("debug", __EXT_PACKAGE + ": " + Array.prototype.join.call(arguments, " ")); },
};

// deckboard-extension-kit API: constructor(name, inputs, execute)
function __DeckboardExtension(moduleName, inputs, execute) {
    this.name = moduleName;
    this.inputs = inputs || [];
    this.execute = execute || function () {};
    this.configs = {};
    this.platforms = ["WINDOWS"];
}
__DeckboardExtension.prototype = Object.create(__Extension.prototype);
__DeckboardExtension.prototype.constructor = __DeckboardExtension;

var __kit = {
    Extension: __Extension,
    DeckboardExtension: __DeckboardExtension,
    extensionLog: function (level, msg) { __host_log(String(level || "info"), String(msg)); },
    INPUT_METHOD: __INPUT_METHOD,
    Platforms: __PLATFORMS,
    PLATFORMS: __PLATFORMS,
    InputTypes: {
        text: "input:text",
        key: "input:key",
        select: "input:select",
        file: "input:file",
        folder: "input:folder",
        color: "input:color",
        checkbox: "input:checkbox",
        textarea: "input:multilinetext",
    },
    ICONS: __ICONS,
    log: __kit_log,
};

// ------------------------------------------------------------------ electron
var __electron = {
    dialog: {
        showErrorBox: function (title, msg) { __host_dialog_error(String(title), String(msg)); },
        showMessageBox: function (opts) { __host_dialog_error("Deckboard", JSON.stringify(opts)); },
        showOpenDialog: function () { __host_log("warn", __EXT_PACKAGE + ": electron dialog.showOpenDialog not supported"); },
    },
    app: { getPath: function () { return __EXT_ROOT; } },
    BrowserWindow: function () {
        this.webContents = { send: function () {} };
    },
};


// ---------------------------------------------------------------- http/https
// Node-style API over the synchronous __host_http native. Callbacks fire
// from setTimeout so request() can return before the response exists.
function __url_parse(str) {
    var m = /^(\w+:)\/\/([^\/:?#]+)(?::(\d+))?(\/[^?#]*)?(\?[^#]*)?(#.*)?$/.exec(str) || [];
    return {
        protocol: m[1] || "http:",
        hostname: m[2] || "",
        port: m[3] || "",
        path: (m[4] || "/") + (m[5] || ""),
        href: str,
    };
}

function __http_request(options, cb) {
    var o = typeof options === "string" ? __url_parse(options) : options;
    var url = (o.protocol || "http:") + "//" + (o.hostname || o.host || "") +
        (o.port ? ":" + o.port : "") + (o.path || "/");
    var method = (o.method || "GET").toUpperCase();
    var body = null;
    var events = {};
    var req = {
        setHeader: function () { return req; },
        removeHeader: function () { return req; },
        setTimeout: function () { return req; },
        write: function (chunk) { body = (body === null ? "" : body) + chunk; return req; },
        end: function (chunk) {
            if (chunk !== undefined) req.write(chunk);
            setTimeout(function () {
                var res = __host_http(JSON.stringify({ url: url, method: method, body: body }));
                var parsed = {};
                try { parsed = JSON.parse(res); } catch (e) { parsed = { error: String(res) }; }
                if (parsed.error && parsed.status === 0) {
                    __emit(events, "error", [new Error(parsed.error)]);
                    return;
                }
                var resp_events = {};
                var resp = {
                    statusCode: parsed.status || 0,
                    headers: { "content-type": ["application/json"] },
                    setEncoding: function () { return resp; },
                    on: function (ev, f) { (resp_events[ev] = resp_events[resp_events ? ev : ev] || (resp_events[ev] = [])).push(f); return resp; },
                };
                if (typeof cb === "function") cb(resp);
                __emit(events, "response", [resp]);
                setTimeout(function () {
                    __emit(resp_events, "data", [parsed.body === undefined ? "" : parsed.body]);
                }, 0);
                setTimeout(function () {
                    __emit(resp_events, "end", []);
                }, 0);
            }, 0);
            return req;
        },
        on: function (ev, f) { (events[ev] = events[ev] || []).push(f); return req; },
        once: function (ev, f) { return req.on(ev, f); },
        destroy: function () {},
    };
    return req;
}

function __emit(events, ev, args) {
    var list = events[ev] || [];
    for (var i = 0; i < list.length; i++) {
        try { list[i].apply(null, args); } catch (e) { __host_log("error", "http event handler: " + e); }
    }
}

var __http_mod = {
    request: function (options, cb) { return __http_request(options, cb); },
    get: function (options, cb) {
        var o = typeof options === "string" ? __url_parse(options) : options;
        o.method = "GET";
        return __http_request(o, cb);
    },
};
__http_mod.ClientRequest = function () {};

var __http_url_mod = { parse: __url_parse, format: function (u) { return u.href || String(u); } };

var global = typeof globalThis !== "undefined" ? globalThis : this;
var __debug_fn = function (ns) {
    var f = function () { __host_log("debug", "[" + ns + "] " + __console_fmt(arguments)); };
    f.enabled = true;
    f.namespace = ns;
    return f;
};
__debug_fn.enable = function () {};
__debug_fn.disable = function () {};
__debug_fn.namespaces = "";
__debug_fn.colors = [];
__debug_fn.log = console.error;
var __constants_mod = {};
var __assert_mod = function (cond, msg) { if (!cond) throw new Error(msg || "assertion failed"); };
__assert_mod.ok = __assert_mod;
__assert_mod.equal = function (a, b, m) { if (a != b) throw new Error(m || "assert.equal failed"); };
__assert_mod.strictEqual = function (a, b, m) { if (a !== b) throw new Error(m || "assert.strictEqual failed"); };
__assert_mod.fail = function (m) { throw new Error(m || "assert.fail"); };

function __resolve_module(spec, from_dir) {
    // relative to the requiring module
    if (spec.charAt(0) === "." || spec.charAt(0) === "/" || /^[a-zA-Z]:/.test(spec) || spec.indexOf("\\") === 0) {
        var base = __path.normalize(from_dir + "/" + spec);
        var tries = [base, base + ".js", base + ".json", base + "/index.js", base + "/package.json"];
        for (var i = 0; i < tries.length; i++) {
            if (__host_file_exists(tries[i])) return __as_file(tries[i]);
        }
        return null;
    }
    // the real graceful-fs patches fs methods our shim does not have;
    // our builtin alias covers the API surface fs-extra actually uses.
    // Same idea for deckboard-kit: extensions ship a copy that drags in
    // electron-log, while our builtin shim is the intended interface.
    if (spec === "graceful-fs" || spec === "deckboard-kit" || spec === "deckboard-extension-kit") return null;
    // node_modules lookup from the package root
    var nm = __EXT_ROOT + "/node_modules/" + spec;
    var pkg = nm + "/package.json";
    var tries2 = [];
    if (__host_file_exists(pkg)) {
        try {
            var pj = JSON.parse(__host_read_file(pkg));
            if (pj && pj.main) tries2.push(nm + "/" + pj.main);
        } catch (e) {}
    }
    tries2.push(nm + "/index.js", nm + "/dist/index.js", nm + "/lib/index.js", nm + ".js");
    for (var j = 0; j < tries2.length; j++) {
        if (__host_file_exists(tries2[j])) return __as_file(tries2[j]);
    }
    return null;
}


// ---------------------------------------------------------------- xml2js
// Minimal parser covering wmic XML output for node-wmi. Callback style:
// parseString(xml, [opts,] cb)
function __xml_parse_node(str, pos, opts) {
    // str starts right after a "<tag ...>"; returns [obj, end_pos]
    var gt = str.indexOf(">", pos);
    if (gt < 0) return [{}, str.length];
    var tag = /^<([\w:.-]+)/.exec(str.slice(pos - 1));
    var name = tag ? tag[1] : "?";
    var attr_str = str.slice(pos - 1 + name.length + 1, gt);
    var self_closing = str[gt - 1] === "/";
    if (self_closing) gt -= 1;
    var attrs = {};
    var am, are = /([\w:.-]+)="([^"]*)"/g;
    while ((am = are.exec(attr_str)) !== null) attrs[am[1]] = am[2];
    var children = [];
    var text = "";
    var p = gt + 1;
    if (!self_closing) {
        var close_re = new RegExp("</" + name.replace(/[:.]/g, "\.") + "\s*>");
        while (p < str.length) {
            var lt = str.indexOf("<", p);
            if (lt < 0) { text += str.slice(p); p = str.length; break; }
            if (lt > p) text += str.slice(p, lt);
            if (str[lt + 1] === "/") {
                p = lt;
                break;
            }
            if (str.slice(lt, lt + 9) === "<![CDATA[") {
                var cend = str.indexOf("]]>", lt);
                text += str.slice(lt + 9, cend < 0 ? str.length : cend);
                p = (cend < 0 ? str.length : cend) + 3;
                continue;
            }
            var child = __xml_parse_node(str, lt + 1, opts);
            if (child[0] !== null) children.push(child[0]);
            p = child[1];
        }
        var cm = close_re.exec(str.slice(p));
        p = cm ? p + cm.index + cm[0].length : str.length;
    }
    var obj = {};
    if (Object.keys(attrs).length) obj.$ = attrs;
    for (var i = 0; i < children.length; i++) {
        var c = children[i];
        for (var k in c) {
            if (opts && opts.explicitArray === false) {
                if (obj[k] === undefined) obj[k] = c[k];
                else if (Array.isArray(obj[k])) obj[k].push(c[k]);
                else obj[k] = [obj[k], c[k]];
            } else {
                if (!Array.isArray(obj[k])) obj[k] = [];
                if (Array.isArray(c[k]) && c[k].length === 1) obj[k].push(c[k][0]);
                else obj[k].push(c[k]);
            }
        }
    }
    if (children.length === 0 && text.trim() !== "") obj._ = text;
    var out = {};
    out[name] = obj;
    return [out, p];
}

var __xml2js = {
    Parser: function (opts) { this._opts = opts || {}; },
    parseString: function (xml, a, b) {
        var opts = typeof a === "object" ? a : {};
        var cb = typeof a === "function" ? a : b;
        try {
            var parsed = {};
            var p = 0;
            xml = String(xml).replace(/<[?]([\s\S]*?)[?]>/g, "");
            while (p < xml.length) {
                var lt = xml.indexOf("<", p);
                if (lt < 0) break;
                if (xml[lt + 1] === "!") { p = xml.indexOf(">", lt) + 1; continue; }
                var r = __xml_parse_node(xml, lt + 1, opts);
                for (var k in r[0]) parsed[k] = r[0][k];
                p = r[1];
            }
            if (typeof cb === "function") cb(null, parsed);
            return parsed;
        } catch (e) {
            if (typeof cb === "function") cb(e, null);
        }
    },
    parseStringPromise: function (xml, opts) {
        return new Promise(function (res, rej) {
            __xml2js.parseString(xml, opts || {}, function (e, r) { if (e) rej(e); else res(r); });
        });
    },
};
__xml2js.Parser.prototype.parseString = function (xml, cb) {
    __xml2js.parseString(xml, this._opts, cb);
};

// ------------------------------------------------------------ module loader
var __builtin_modules = {
    "deckboard-kit": __kit,
    "deckboard-extension-kit": __kit,
    "child_process": __child_process,
    "node-fetch": __fetch,
    "electron": __electron,
    "fs": __fs,
    "fs-extra": __fs_extra,
    "graceful-fs": __fs,
    "path": __path_mod,
    "os": __os,
    "util": __util,
    "events": __EventEmitter,
    "opn": __opn,
    "open": __opn,
    "wintools": __wintools,
    "win-tools": __wintools,
    "node-wmi": __wmi_query,
    "process": __process,
    "buffer": { Buffer: __Buffer, Buffer2: __Buffer },
    "zlib": {
        inflateSync: function (buf) { return buf; },
        gunzipSync: function (buf) { return buf; },
        deflateSync: function (buf) { return buf; },
        inflate: function (buf, cb) { if (typeof cb === "function") cb(null, buf); },
        gunzip: function (buf, cb) { if (typeof cb === "function") cb(null, buf); },
    },
    "tty": { isatty: function () { return false; }, setRawMode: function () {} },
    "net": {
        connect: function () { throw new Error("net sockets are not available in this host"); },
        createConnection: function () { throw new Error("net sockets are not available in this host"); },
        createServer: function () { throw new Error("net servers are not available in this host"); },
    },
    "xml2js": __xml2js,
    "debug": __debug_fn,
    "constants": __constants_mod,
    "assert": __assert_mod,
    "punycode": {
        decode: function (s) { return s; },
        encode: function (s) { return s; },
        toASCII: function (s) { return s; },
        toUnicode: function (s) { return s; },
        ucs2: { decode: function (s) { return s.split(""); }, encode: function (a) { return a.join(""); } },
        version: "2.3.1",
    },
    "http": __http_mod,
    "https": __http_mod,
    "url": __http_url_mod,
    "stream": {
        Readable: __EventEmitter,
        Writable: __EventEmitter,
        Duplex: __EventEmitter,
        Transform: __EventEmitter,
        PassThrough: __EventEmitter,
        Stream: __EventEmitter,
    },
    "timers": {
        setTimeout: setTimeout,
        setInterval: setInterval,
        clearTimeout: clearTimeout,
        clearInterval: clearInterval,
        setImmediate: function (cb) { return setTimeout(cb, 0); },
    },
};

var __module_cache = {};
var __module_stack = [];
var __ext_exports = null;
// extensions assign the instance directly (e.g. `__ext_instance = ...`),
// so the global must be pre-declared
var __ext_instance = null;
// many extensions use `process` as a bare global
var process = __process;
// some bundles reference the global object directly
// exists() is true for directories too; module loading needs a file, and
// node semantics turn a directory module into its index.js
function __as_file(p) {
    if (__host_file_exists(p + "/index.js")) return p + "/index.js";
    return p;
}

function __dirname_of(p) {
    return __path.dirname(p);
}

function __load_module(abs_path) {
    if (__module_cache[abs_path] !== undefined) return __module_cache[abs_path].exports;
    if (abs_path.endsWith(".json")) {
        var parsed = JSON.parse(__host_read_file(abs_path));
        __module_cache[abs_path] = { exports: parsed };
        return parsed;
    }
    var code = __host_read_file(abs_path);
    if (code === null) throw new Error("cannot read module file: " + abs_path);
    var module_obj = { exports: {} };
    __module_cache[abs_path] = module_obj;
    var module_dir = __dirname_of(abs_path);
    var local_require = function (spec) {
        var resolved = __resolve_module(spec, module_dir);
        if (resolved === null) {
            if (__builtin_modules[spec] !== undefined) return __builtin_modules[spec];
            throw new Error("deckboard-ext: module '" + spec + "' is not available (package " + __EXT_PACKAGE + ")");
        }
        return __load_module(resolved);
    };
    var fn = new Function(
        "exports", "require", "module", "__filename", "__dirname",
        code + "\n"
    );
    __module_stack.push(abs_path);
    try {
        fn.call(module_obj.exports, module_obj.exports, local_require, module_obj, abs_path, module_dir);
    } catch (e) {
        throw new Error("loading [" + __module_stack.join(" > ") + "]: " + (e && e.message || e) + (e && e.stack ? " @ " + e.stack : ""));
    } finally {
        __module_stack.pop();
    }
    return module_obj.exports;
}

function __require_entry() {
    var entry = __EXT_ROOT + "/index.js";
    __ext_exports = __load_module(entry);
    var inst = __ext_exports;
    // old-style factory: module.exports = ({dialog, setValue}) => instance
    if (typeof inst === "function") {
        inst = inst({
            dialog: __electron.dialog,
            setValue: function (obj) { __pending_set_values.push(obj); },
            log: __kit_log,
        });
    }
    if (inst && inst.default && typeof inst.default === "object") inst = inst.default;
    if (!inst || (typeof inst.execute !== "function" && typeof inst.initExtension !== "function")) {
        throw new Error("entry did not produce an extension instance");
    }
    __ext_instance = inst;
    return inst;
}
