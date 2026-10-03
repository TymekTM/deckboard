//! BoardCache tests: the display-only offline snapshot survives a
//! roundtrip, and junk never crashes the launch path.

package app.pulpit.mobile

import app.pulpit.mobile.proto.Board
import app.pulpit.mobile.proto.Tile
import app.pulpit.mobile.state.BoardCache
import app.pulpit.mobile.state.CachePayload
import app.pulpit.mobile.state.decodeCache
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

class BoardCacheTest {
    @get:Rule
    val tmp = TemporaryFolder()

    @Test
    fun roundtripKeepsBoardsValuesAndSeries() {
        val cache = BoardCache(tmp.newFolder())
        val board = Board(id = 7, name = "All In One", width = 6, height = 4, order = 2)
        val payload = CachePayload(
            boards = listOf(board),
            values = mapOf(
                "ext.volume" to JsonPrimitive(42),
                "ext.label" to JsonPrimitive("GLM 5h"),
            ),
            series = mapOf("ext.chart" to listOf(1.5, 2.5, 3.0)),
        )
        cache.save(payload)
        val loaded = cache.load()
        assertEquals(payload, loaded)
        // a JSON primitive number stays a primitive after the roundtrip
        assertEquals(42, loaded?.values?.get("ext.volume")?.jsonPrimitive?.content?.toInt())
    }

    @Test
    fun junkReadsAsNullNotCrash() {
        val dir = tmp.newFolder()
        val cache = BoardCache(dir)
        java.io.File(dir, "board-cache.json").writeText("{\"boards\": [torn")
        assertNull(cache.load())
        // a missing file too
        assertNull(BoardCache(tmp.newFolder()).load())
    }

    @Test
    fun unknownFutureFieldsAreIgnored() {
        // the desktop may grow fields before the tablet updates; the
        // display-only cache must not brick the restore path
        val text = """{"boards":[],"values":{},"series":{},"futureField":1}"""
        val decoded = decodeCache(text)
        assertTrue(decoded?.boards?.isEmpty() == true)
    }

    @Test
    fun clearRemovesTheFile() {
        val cache = BoardCache(tmp.newFolder())
        cache.save(CachePayload(emptyList(), emptyMap(), emptyMap()))
        cache.clear()
        assertNull(cache.load())
    }

    @Test
    fun cacheCarriesTilesSoTheGridRendersOffline() {
        val dir = tmp.newFolder()
        val cache = BoardCache(dir)
        val tileJson = buildJsonObject {
            put("id", 1L)
            put("kind", "button")
        }
        val board = Board(
            id = 1,
            tiles = listOf(Json.decodeFromString(Tile.serializer(), tileJson.toString())),
        )
        cache.save(CachePayload(listOf(board), emptyMap(), emptyMap()))
        assertEquals(1, cache.load()?.boards?.single()?.tiles?.size)
    }
}
