//! Display-only offline cache: the last boards snapshot plus channel
//! state, saved on every sync so a cold launch shows the board instantly
//! while the link reconnects (ROADMAP M4 "offline cache"). Never used as
//! state truth: every connect gets a full snapshot from the server
//! (docs/protocol-v2.md), so there is nothing to invalidate - the file
//! is overwritten whole, or deleted on unpair.

package app.pulpit.mobile.state

import app.pulpit.mobile.proto.Board
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import java.io.File

/** The persisted slice of UI state. Boards keep the wire shape (they
 *  are @Serializable already); values/series mirror the ViewModel maps. */
@Serializable
data class CachePayload(
    val boards: List<Board>,
    val values: Map<String, JsonElement>,
    val series: Map<String, List<Double>>,
)

/** Pure decode: junk (torn write, schema drift) reads as null, never as
 *  a crash - the cache is a convenience, not a source of truth. */
fun decodeCache(text: String): CachePayload? =
    runCatching {
        Json { ignoreUnknownKeys = true }.decodeFromString<CachePayload>(text)
    }.getOrNull()

class BoardCache(private val dir: File) {
    private val file = File(dir, "board-cache.json")
    private val json = Json { ignoreUnknownKeys = true }

    fun load(): CachePayload? {
        val text = runCatching { file.readText() }.getOrNull() ?: return null
        return decodeCache(text)
    }

    /** Whole-file rewrite on a caller-chosen worker thread; decodeCache
     *  absorbs a torn result, and the `.tmp` rename keeps an empty file
     *  from ever appearing under the real name. */
    fun save(payload: CachePayload) {
        runCatching {
            dir.mkdirs()
            val tmp = File(dir, "board-cache.tmp")
            tmp.writeText(json.encodeToString(payload))
            if (!tmp.renameTo(file)) {
                file.delete()
                tmp.renameTo(file)
            }
        }
    }

    fun clear() {
        runCatching { file.delete() }
    }
}
