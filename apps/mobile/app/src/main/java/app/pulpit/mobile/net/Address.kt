//! Address validation for the connect path (round 4, MOB-01): a host
//! OkHttp cannot parse used to escape as an IllegalArgumentException
//! from Request.Builder().url() straight through the click handler - and
//! since the bad host was already saved, as a launch crash loop on every
//! later start. The check uses the same parser OkHttp will use, so
//! whatever passes here cannot throw later.

package app.pulpit.mobile.net

import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

/** Why `host:port` cannot become a socket URL; null when it can.
 *  Port 0 stands for "no parseable number" from the text field.
 *  The check parses the `http` form of the URL because that is exactly
 *  what OkHttp does internally: Request.Builder.url() rewrites
 *  ws/wss to http/https before HttpUrl parses the authority. */
fun addressError(host: String, port: Int): String? {
    if (host.isBlank()) return "enter the PC address"
    if (port !in 1..65535) return "port must be between 1 and 65535"
    // parsing alone is not enough: "192.168.1.2/x" parses as host
    // 192.168.1.2 with the port swallowed into the path, so the parsed
    // authority must be exactly what was typed
    val url = "http://$host:$port/v2/ws".toHttpUrlOrNull()
    val typed = host.trim().removePrefix("[").removeSuffix("]")
    return if (url == null || !url.host.equals(typed, ignoreCase = true) || url.port != port) {
        "\"$host\" is not a valid address"
    } else {
        null
    }
}
