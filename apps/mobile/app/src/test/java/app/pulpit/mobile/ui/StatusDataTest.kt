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
    fun planWindowsFilterDropsUntickedRowsAndRecomputesTheSummary() {
        // MOB-06: params.windows (normalized server-side from the
        // legacy "windows:5h,week" options token) drops the unticked
        // window's rows, and the summary names the worst VISIBLE
        // percent row instead of the producer's raw line
        val payload = """
            {
              "rows": [
                {"label": "PLANS", "state": "header"},
                {"label": "GLM 5h", "value": "62.1M", "state": "ok", "percent": 62.4},
                {"label": "GLM week", "value": "31.0M", "state": "ok", "percent": 31.2}
              ],
              "summary": "GLM week 31%"
            }
        """.trimIndent()
        val tile = Tile(
            id = 1,
            kind = "list",
            params = json.parseToJsonElement("""{"windows": ["5h"]}"""),
        )
        val data = statusData(tile, json.parseToJsonElement(payload))!!
        assertEquals(listOf("PLANS", "GLM 5h"), data.rows.map { it.label })
        assertEquals("GLM 5h 62%", data.summary)

        // no filter on the wire: both rows stay and the summary
        // recomputes from the worst visible percent row (the desktop
        // recomputes whenever any percent row is visible, filter or not)
        val plain = statusData(Tile(id = 1, kind = "list"), json.parseToJsonElement(payload))!!
        assertEquals(3, plain.rows.size)
        assertEquals("GLM 5h 62%", plain.summary)

        // empty filter (both unticked): every plan row hides, headers
        // stay, and with no visible percent row the producer summary is
        // kept as-is
        val both = Tile(
            id = 1,
            kind = "list",
            params = json.parseToJsonElement("""{"windows": []}"""),
        )
        val filtered = statusData(both, json.parseToJsonElement(payload))!!
        assertEquals(listOf("PLANS"), filtered.rows.map { it.label })
        assertEquals("GLM week 31%", filtered.summary)
    }

    @Test
    fun hideSummarySilencesTheLineEvenWhenRecomputing() {
        val tile = Tile(id = 1, kind = "list")
        val payload = """
            {
              "rows": [{"label": "GLM 5h", "state": "ok", "percent": 62.4}],
              "summary": "GLM 5h 62%",
              "hide_summary": true
            }
        """.trimIndent()
        val data = statusData(tile, json.parseToJsonElement(payload))!!
        assertEquals("", data.summary)
    }

    @Test
    fun laneProviderMapsGlmToZcode() {
        assertEquals("zcode", laneProvider("GLM 5h"))
        assertEquals("claude", laneProvider("Claude week"))
        assertEquals("", laneProvider(""))
    }

    /** The spotify-now-playing round-trip (design §4): the optional
     *  `image` hash and `progress` object survive the parser; `compact`
     *  arrives as a string here, which reads as no compact entries. */
    @Test
    fun parsesImageAndProgress() {
        val data = statusData(
            tile = Tile(id = 1, kind = "list"),
            live = json.parseToJsonElement(
                """
                {
                  "title": "Spotify",
                  "rows": [
                    {"label": "Track", "value": "Song"},
                    {"label": "Artist", "value": "A, B"}
                  ],
                  "compact": "Song — Artist",
                  "summary": "Song — Artist",
                  "image": "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
                  "progress": {"position_ms": 61234, "duration_ms": 201000, "playing": true}
                }
                """.trimIndent(),
            ),
        )!!
        assertEquals(2, data.rows.size)
        assertEquals(
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
            data.image,
        )
        assertEquals(StatusProgress(61234, 201000, playing = true), data.progress)
        assertEquals(0, data.compact.size)
        assertEquals("Song — Artist", data.summary)
    }

    @Test
    fun payloadsWithoutImageOrProgressParseAsBefore() {
        // ai-dev pushes (and "nothing playing" spotify pushes) carry
        // neither field - they must keep parsing exactly as before
        val data = statusData(
            tile = Tile(id = 1, kind = "list"),
            live = json.parseToJsonElement(
                """{"rows": [{"label": "Status", "value": "Nothing playing"}],
                   "compact": "Nothing playing", "summary": "Nothing playing"}""",
            ),
        )!!
        assertNull(data.image)
        assertNull(data.progress)
    }

    @Test
    fun garbageImageAndProgressReadAsNull() {
        val tile = Tile(id = 1, kind = "list")
        val garbage = statusData(
            tile = tile,
            live = json.parseToJsonElement(
                """
                {
                  "rows": [{"label": "Track", "value": "Song"}],
                  "image": "",
                  "progress": {"position_ms": "x", "duration_ms": 201000, "playing": true}
                }
                """.trimIndent(),
            ),
        )!!
        assertNull(garbage.image)
        assertNull(garbage.progress)

        val junk = statusData(
            tile = tile,
            live = json.parseToJsonElement(
                """
                {
                  "rows": [{"label": "Track", "value": "Song"}],
                  "image": 42,
                  "progress": {"position_ms": -5, "duration_ms": 0, "playing": "yes"}
                }
                """.trimIndent(),
            ),
        )!!
        // 42 is a string primitive, not a hash shape - dropped; duration
        // 0 can never be a valid track - dropped
        assertNull(junk.image)
        assertNull(junk.progress)

        val shapes = statusData(
            tile = tile,
            live = json.parseToJsonElement(
                """
                {
                  "rows": [{"label": "Track", "value": "Song"}],
                  "image": {"hash": "9f86d081"},
                  "progress": [61234, 201000, true]
                }
                """.trimIndent(),
            ),
        )!!
        assertNull(shapes.image)
        assertNull(shapes.progress)
    }
}
