package app.pulpit.mobile.proto

import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** boards.delta op decoding: unknown ops are skipped, malformed ones
 *  reject the batch so the caller can force a snapshot resync. */
class DeltaOpsTest {

    private val json = Json { ignoreUnknownKeys = true; isLenient = true }

    private fun fixtureOps(name: String): List<kotlinx.serialization.json.JsonElement> {
        val loader = checkNotNull(javaClass.classLoader) { "no test class loader" }
        val stream = loader.getResourceAsStream(name)
            ?: error("fixture $name not on the test classpath")
        val frame = json.decodeFromString(Frame.serializer(), stream.readBytes().decodeToString())
        return json.decodeFromString(BoardsDelta.serializer(), frame.payload.toString()).ops
    }

    @Test
    fun fixtureOpsDecode() {
        val ops = decodeDeltaOps(fixtureOps("boards.delta.json"), json)!!
        assertEquals(2, ops.size)
        val tileSet = ops[0] as BoardOp.TileSet
        assertEquals(17L, tileSet.tile.id)
        assertEquals(BoardOp.BoardRemove(9L), ops[1])
    }

    @Test
    fun unknownOpNameIsSkipped() {
        val raw = """{"op":"tile-frobnicate","board":3}"""
        val ops = decodeDeltaOps(listOf(json.parseToJsonElement(raw)), json)!!
        assertEquals(0, ops.size)
    }

    @Test
    fun malformedKnownOpRejectsTheBatch() {
        // tile-set with a tile whose fields do not decode: the batch is
        // rejected (null) so the client resyncs instead of drifting
        val good = """{"op":"board-remove","board":9}"""
        val bad = """{"op":"tile-set","board":3,"tile":{"id":"nope","x":"also-nope"}}"""
        val ops = decodeDeltaOps(
            listOf(json.parseToJsonElement(good), json.parseToJsonElement(bad)),
            json,
        )
        assertNull(ops)
    }
}
