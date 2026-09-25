package app.pulpit.mobile.ui

import app.pulpit.mobile.proto.Tile
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** The ai-dev status payload parser (Widgets.statusData): rows/compact/
 *  summary come off the tile's live channel; a non-object value (plain
 *  array, scalar) must return null so the tile falls back to the plain
 *  option list. */
class StatusDataTest {

    private val json = Json { ignoreUnknownKeys = true; isLenient = true }

    @Test
    fun parsesRowsCompactAndSummary() {
        val data = statusData(
            tile = Tile(id = 1, kind = "list"),
            live = json.parseToJsonElement(
                """
                {
                  "title": "AI dev work",
                  "rows": [
                    {"label": "AGENTS", "state": "header"},
                    {"label": "fix editor", "value": "working", "state": "working",
                     "provider": "claude"},
                    {"label": "GLM 5h", "value": "62.1M", "state": "ok", "percent": 62.4},
                    {"label": "wait review", "value": "", "state": "attention"}
                  ],
                  "compact": [
                    {"provider": "claude", "count": 2, "state": "working"},
                    {"provider": "zcode", "count": 1}
                  ],
                  "summary": "GLM 5h 62%"
                }
                """.trimIndent(),
            ),
        )!!
        assertEquals(4, data.rows.size)
        assertEquals("header", data.rows[0].state)
        assertEquals(true, data.rows[0].isHeader)
        assertEquals("claude", data.rows[1].provider)
        assertEquals(62.4, data.rows[2].percent!!, 1e-9)
        assertEquals("attention", data.rows[3].state)
        assertEquals(2, data.compact.size)
        assertEquals(1, data.compact[1].count)
        assertEquals("working", data.compact[1].state)
        assertEquals("GLM 5h 62%", data.summary)
    }

    @Test
    fun missingRowsReturnsNull() {
        val tile = Tile(id = 1, kind = "list")
        assertNull(statusData(tile, json.parseToJsonElement("""{"summary": "x"}""")))
        assertNull(statusData(tile, json.parseToJsonElement("""["one", "two"]""")))
        assertNull(statusData(tile, json.parseToJsonElement("""12""")))
        assertNull(statusData(tile, null))
    }

    @Test
    fun rowDefaultsAreLenient() {
        val data = statusData(
            tile = Tile(id = 1, kind = "list"),
            live = json.parseToJsonElement("""{"rows": [{}]}"""),
        )!!
        assertEquals(1, data.rows.size)
        assertEquals("off", data.rows[0].state)
        assertEquals(false, data.rows[0].isHeader)
        assertEquals(null, data.rows[0].percent)
        assertEquals(0, data.compact.size)
        assertEquals("", data.summary)
    }

    @Test
    fun laneProviderMapsGlmToZcode() {
        assertEquals("zcode", laneProvider("GLM 5h"))
        assertEquals("claude", laneProvider("Claude week"))
        assertEquals("", laneProvider(""))
    }
}
