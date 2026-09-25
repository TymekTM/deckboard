package app.pulpit.mobile.proto

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Golden fixture contract: the same files the Rust side round-trips
 * (`crates/proto/tests/fixtures`, wired in as test resources) must parse
 * into the Kotlin v2 wire models. Any wire drift fails both builds.
 */
class ProtoFixturesTest {

    private val json = Json { ignoreUnknownKeys = true; isLenient = true }

    private fun fixture(name: String): Frame {
        val stream = javaClass.classLoader.getResourceAsStream(name)
            ?: error("fixture $name not on the test classpath")
        return json.decodeFromString(Frame.serializer(), stream.readBytes().decodeToString())
    }

    private inline fun <reified T> typed(frame: Frame, kind: String): T {
        assertEquals(kind, frame.type)
        return json.decodeFromString(kotlinx.serialization.serializer(), frame.payload.toString())
    }

    @Test
    fun envelopeCarriesProtocolVersion() {
        for (name in listOf("hello", "welcome", "error", "boards.sync", "boards.delta",
                "board.open", "state.sync", "state.patch", "interaction", "server.shutdown")) {
            val frame = fixture("$name.json")
            assertEquals("fixture $name", 2, frame.v)
            assertTrue("fixture $name", frame.type.isNotEmpty())
        }
    }

    @Test
    fun helloParses() {
        val frame = fixture("hello.json")
        assertEquals("hello", frame.type)
        assertEquals("h1", frame.id)
        val hello = json.decodeFromString(Hello.serializer(), frame.payload.toString())
        assertEquals("pulpit-mobile", hello.client)
        assertEquals("Tablet salon", hello.name)
        assertEquals(listOf("graph", "list"), hello.capabilities)
    }

    @Test
    fun welcomeParses() {
        val frame = fixture("welcome.json")
        assertEquals("welcome", frame.type)
        assertEquals("h1", frame.ack)
        val welcome = json.decodeFromString(Welcome.serializer(), frame.payload.toString())
        assertEquals(2, welcome.protocol)
        assertEquals(7L, welcome.generation)
        assertEquals("Tablet salon", welcome.device.name)
        // fixture is a token reconnect: no secret on the wire
        assertNull(welcome.token)
        assertEquals(V2.SHAPE_SERIES, welcome.channels["ext.si-cpu-usage"]!!.shape)
        assertEquals(120, welcome.channels["ext.si-cpu-usage"]!!.cap)
        assertEquals(V2.SHAPE_SCALAR, welcome.channels["ext.speaker-muted"]!!.shape)
    }

    @Test
    fun errorParses() {
        val frame = fixture("error.json")
        assertEquals("error", frame.type)
        val error = json.decodeFromString(ErrorPayload.serializer(), frame.payload.toString())
        assertEquals("unknown-tile", error.code)
        assertEquals("no tile 42", error.message)
    }

    @Test
    fun boardsSyncParses() {
        val frame = fixture("boards.sync.json")
        assertEquals("boards.sync", frame.type)
        assertNull(frame.ack) // server push
        val sync = typed<BoardsSync>(frame, V2.TYPE_BOARDS_SYNC)
        assertEquals(7L, sync.generation)
        assertEquals(1, sync.boards.size)
        val board = sync.boards[0]
        assertEquals(3L, board.id)
        assertEquals(Background.color("#2c3e50"), board.background)
        assertEquals(2, board.tiles.size)

        val button = board.tiles[0]
        assertEquals(17L, button.id)
        assertEquals(0, button.x)
        assertEquals(V2.KIND_BUTTON, button.kind)
        assertEquals("ext.speaker-muted", button.state!!.channel)
        assertEquals(V2.SHAPE_SCALAR, button.state!!.shape)
        assertEquals("#F5AB35", button.style!!.color)
        assertEquals("\uf026", button.style!!.icon)
        assertEquals("Mute", button.style!!.title)

        val slider = board.tiles[1]
        assertEquals(V2.KIND_SLIDER, slider.kind)
        assertEquals(2, slider.h)
        assertEquals(listOf(V2.INT_SLIDE), slider.interactions)
    }

    @Test
    fun boardsDeltaParses() {
        val frame = fixture("boards.delta.json")
        assertEquals("boards.delta", frame.type)
        val delta = typed<BoardsDelta>(frame, V2.TYPE_BOARDS_DELTA)
        assertEquals(8L, delta.generation)
        assertEquals(2, delta.ops.size)

        val ops = delta.ops.mapNotNull { BoardOp.from(it, json) }
        assertEquals(2, ops.size)
        val tileSet = ops[0] as BoardOp.TileSet
        assertEquals(3L, tileSet.boardId)
        assertEquals(17L, tileSet.tile.id)
        assertTrue(tileSet.tile.interacts(V2.INT_PRESS_START))
        assertEquals(BoardOp.BoardRemove(9L), ops[1])
    }

    @Test
    fun stateSyncParses() {
        val frame = fixture("state.sync.json")
        val sync = typed<StateSync>(frame, V2.TYPE_STATE_SYNC)
        assertEquals("OFF", sync.values["ext.speaker-muted"]!!.jsonPrimitive.content)
        assertEquals(true, sync.values["discord.microphone-muted"]!!.jsonPrimitive.boolean)
        assertEquals(listOf(0.1, 0.42, 0.44), sync.series["ext.si-cpu-usage"])
    }

    @Test
    fun statePatchParses() {
        val frame = fixture("state.patch.json")
        assertEquals("state.patch", frame.type)
        val patch = typed<StatePatch>(frame, V2.TYPE_STATE_PATCH)
        assertEquals(3, patch.changes.size)
        assertEquals("discord.microphone-muted", patch.changes[0].channel)
        assertEquals(false, patch.changes[0].value.jsonPrimitive.boolean)
    }

    @Test
    fun interactionParses() {
        val frame = fixture("interaction.json")
        assertEquals("interaction", frame.type)
        assertEquals("i9", frame.id)
        val interaction = typed<InteractionPayload>(frame, V2.TYPE_INTERACTION)
        assertEquals(3L, interaction.board)
        assertEquals(21L, interaction.tile)
        assertEquals(V2.INT_SLIDE, interaction.interaction)
        assertEquals(0.5, interaction.args.value!!, 1e-9)
    }
}
