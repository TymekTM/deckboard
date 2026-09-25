//! Pure board-list mutation for `boards.delta` (docs/protocol-v2.md §4):
//! ops reshape the snapshot the server already sent. The ViewModel calls
//! this and then decides which board stays selected.

package app.pulpit.mobile.state

import app.pulpit.mobile.proto.Board
import app.pulpit.mobile.proto.BoardOp

/** Apply [ops] to [boards]; unknown boards/tiles are ignored (the next
 *  full `boards.sync` re-synchronizes anyway). New boards append. */
internal fun applyBoardOps(boards: List<Board>, ops: List<BoardOp>): List<Board> {
    val result = boards.toMutableList()
    fun boardIndex(id: Long) = result.indexOfFirst { it.id == id }

    for (op in ops) {
        when (op) {
            is BoardOp.BoardSet -> {
                val at = boardIndex(op.board.id)
                if (at >= 0) result[at] = op.board else result.add(op.board)
            }
            is BoardOp.BoardRemove -> {
                val at = boardIndex(op.boardId)
                if (at >= 0) result.removeAt(at)
            }
            is BoardOp.TileSet -> {
                val at = boardIndex(op.boardId)
                if (at >= 0) {
                    val board = result[at]
                    val tiles = board.tiles.toMutableList()
                    val tileAt = tiles.indexOfFirst { it.id == op.tile.id }
                    if (tileAt >= 0) tiles[tileAt] = op.tile else tiles.add(op.tile)
                    result[at] = board.copy(tiles = tiles)
                }
            }
            is BoardOp.TileRemove -> {
                val at = boardIndex(op.boardId)
                if (at >= 0) {
                    val board = result[at]
                    result[at] = board.copy(tiles = board.tiles.filterNot { it.id == op.tileId })
                }
            }
            is BoardOp.TileClear -> {
                val at = boardIndex(op.boardId)
                if (at >= 0) {
                    val board = result[at]
                    result[at] = board.copy(tiles = emptyList())
                }
            }
        }
    }
    return result
}
