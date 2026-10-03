//! Structured logging (ROADMAP M4): one line per event, levelled and
//! tagged, to logcat AND a small rotating file under filesDir/logs so a
//! field problem on the deck tablet leaves evidence. The formatter and
//! the rotation selection are pure and unit-tested; the I/O runs on one
//! daemon thread and is best-effort - logging must never take the deck
//! down, and it never logs token material (ADR-008 callers pass none).

package app.pulpit.mobile.state

import android.content.Context
import android.util.Log
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.TimeZone
import java.util.concurrent.Executors

object Plog {
    private val io = Executors.newSingleThreadExecutor { r ->
        Thread(r, "pulpit-log").apply { isDaemon = true }
    }

    private const val KEEP = 3
    private const val MAX_BYTES = 512L * 1024

    @Volatile private var dir: File? = null

    /** Called once from the Application, before any ViewModel exists. */
    fun init(context: Context) {
        dir = File(context.filesDir, "logs")
        prune()
    }

    fun i(tag: String, msg: String) = log(Level.I, tag, msg)
    fun w(tag: String, msg: String) = log(Level.W, tag, msg)
    fun e(tag: String, msg: String) = log(Level.E, tag, msg)

    private enum class Level { I, W, E }

    private fun log(level: Level, tag: String, msg: String) {
        when (level) {
            Level.I -> Log.i(tag, msg)
            Level.W -> Log.w(tag, msg)
            Level.E -> Log.e(tag, msg)
        }
        val d = dir ?: return
        io.execute {
            runCatching {
                d.mkdirs()
                var f = File(d, fileName(Date()))
                if (f.exists() && f.length() > MAX_BYTES) {
                    // freeze the full day file and start a fresh one; the
                    // frozen copy carries a millis suffix so the newest-N
                    // pruning below still sorts it next to its day
                    f.renameTo(File(d, f.name.removeSuffix(".log") + "-${System.currentTimeMillis()}.log"))
                    prune()
                    f = File(d, fileName(Date()))
                }
                f.appendText(formatLine(System.currentTimeMillis(), level.name, tag, msg) + "\n")
            }
        }
    }

    /** Line shape: `2026-10-03T14:21:53Z I PulpitViewModel the message`. */
    fun formatLine(epochMs: Long, level: String, tag: String, msg: String): String {
        val utc = SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss'Z'", Locale.US).apply {
            timeZone = TimeZone.getTimeZone("UTC")
        }
        return "${utc.format(Date(epochMs))} $level $tag $msg"
    }

    /** Active day file name (`2026-10-03.log`). */
    fun fileName(date: Date): String =
        SimpleDateFormat("yyyy-MM-dd", Locale.US).format(date) + ".log"

    /** Which files survive rotation: the newest [keep] names (the names
     *  sort chronologically, frozen files sit next to their day). */
    fun keepNames(names: List<String>, keep: Int): List<String> =
        names.sorted().takeLast(keep)

    private fun prune() {
        val d = dir ?: return
        io.execute {
            runCatching {
                val logs = d.listFiles { f -> f.name.endsWith(".log") }
                    ?.map { it.name } ?: return@execute
                val keep = keepNames(logs, KEEP)
                logs.filter { it !in keep }.forEach { File(d, it).delete() }
            }
        }
    }
}
