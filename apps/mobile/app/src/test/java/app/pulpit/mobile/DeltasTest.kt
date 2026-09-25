package app.pulpit.mobile.state

import app.pulpit.mobile.proto.Board
import app.pulpit.mobile.proto.BoardOp
import app.pulpit.mobile.proto.Tile
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Test

/** The `boards.delta` list mutation (Deltas.kt): ops reshape the snapshot;
 *  unknown boards/tiles are ignored, new ones append. */
class DeltasTest {

    private val json = Json { ignoreUnknownKeys = true; isLenient = true }

    private fun board(id: Long, name: String, tiles: List<Tile> = emptyList()) =
        Board(id = id, name = name, tiles = tiles)

    private fun tile(id: Long) = Tile(id = id, kind = "button")

    private fun op(jsonText: String): BoardOp =
        BoardOp.from(json.parseToJsonElement(jsonText), json)
            ?: error("op not parsed: $jsonText")

    @Test
    fun boardSetReplacesAndAppends() {
        val boards = listOf(board(1, "one"), board(2, "two"))
        val updated = applyBoardOps(
            boards,
            listOf(
                op("""{"op":"board-set","board":{"id":2,"name":"TWO"}}"""),
                op("""{"op":"board-set","board":{"id":3,"name":"three"}}"""),
            ),
        )
        assertEquals(listOf("one", "TWO", "three"), updated.map { it.name })
    }

    @Test
    fun boardRemoveDropsOnlyThatBoard() {
        val boards = listOf(board(1, "one"), board(2, "two"))
        val updated = applyBoardOps(boards, listOf(op("""{"op":"board-remove","board":1}""")))
        assertEquals(listOf(2L), updated.map { it.id })
        // removing an unknown board is a no-op
        assertEquals(updated, applyBoardOps(updated, listOf(op("""{"op":"board-remove","board":99}"""))))
    }

    @Test
    fun tileSetAddsAndReplaces() {
        val boards = listOf(board(1, "one", listOf(tile(10))))
        val updated = applyBoardOps(
            boards,
            listOf(
                op("""{"op":"tile-set","board":1,"tile":{"id":10,"kind":"slider"}}"""),
                op("""{"op":"tile-set","board":1,"tile":{"id":11,"kind":"knob"}}"""),
            ),
        )
        assertEquals(
            listOf(10L to "slider", 11L to "knob"),
            updated[0].tiles.map { it.id to it.kind },
        )
    }

    @Test
    fun tileRemoveAndClear() {
        val boards = listOf(board(1, "one", listOf(tile(10), tile(11))))
        val updated = applyBoardOps(boards, listOf(op("""{"op":"tile-remove","board":1,"tile":10}""")))
        assertEquals(listOf(11L), updated[0].tiles.map { it.id })
        val cleared = applyBoardOps(updated, listOf(op("""{"op":"tile-clear","board":1}""")))
        assertEquals(emptyList<Long>(), cleared[0].tiles.map { it.id })
    }

    @Test
    fun opsForUnknownBoardAreIgnored() {
        val boards = listOf(board(1, "one"))
        val updated = applyBoardOps(
            boards,
            listOf(
                op("""{"op":"tile-set","board":42,"tile":{"id":10}}"""),
                op("""{"op":"tile-remove","board":42,"tile":10}"""),
                op("""{"op":"tile-clear","board":42}"""),
            ),
        )
        assertEquals(boards, updated)
    }
}
