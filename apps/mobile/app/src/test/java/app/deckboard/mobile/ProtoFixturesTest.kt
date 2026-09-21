package app.deckboard.mobile.proto

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.Calendar

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

    private fun Frame.payloadObject(): JsonObject =
        payload?.jsonObject ?: JsonObject(emptyMap())

    @Test
    fun envelopeCarriesProtocolVersion() {
        for (name in listOf("hello", "welcome", "error", "boards.sync", "boards.delta",
                "board.open", "state.sync", "state.patch", "interaction")) {
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
        val hello = json.decodeFromString(Hello.serializer(), frame.payloadObject().toString())
        assertEquals("deckboard-mobile", hello.client)
        assertEquals("Tablet salon", hello.name)
        assertEquals(listOf("graph", "list"), hello.capabilities)
    }

    @Test
    fun welcomeParses() {
        val frame = fixture("welcome.json")
        assertEquals("welcome", frame.type)
        assertEquals("h1", frame.ack)
        val welcome = json.decodeFromString(Welcome.serializer(), frame.payloadObject().toString())
        assertEquals(2, welcome.protocol)
        assertEquals(7L, welcome.generation)
        assertEquals("Tablet salon", welcome.device.name)
        assertEquals(Shapes.SERIES, welcome.channels["ext.si-cpu-usage"]!!.shape)
        assertEquals(120, welcome.channels["ext.si-cpu-usage"]!!.cap)
        assertEquals(Shapes.SCALAR, welcome.channels["ext.speaker-muted"]!!.shape)
    }

    @Test
    fun errorParses() {
        val frame = fixture("error.json")
        assertEquals("error", frame.type)
        val error = json.decodeFromString(ErrorPayload.serializer(), frame.payloadObject().toString())
        assertEquals("unknown-tile", error.code)
        assertEquals("no tile 42", error.message)
    }

    @Test
    fun boardsSyncParses() {
        val frame = fixture("boards.sync.json")
        assertEquals("boards.sync", frame.type)
        assertEquals(null, frame.ack) // server push
        val sync = json.decodeFromString(BoardsSync.serializer(), frame.payloadObject().toString())
        assertEquals(7L, sync.generation)
        assertEquals(1, sync.boards.size)
        val board = sync.boards[0].jsonObject
        assertEquals(3L, board["id"]!!.jsonPrimitive.content.toLong())
        val tiles = board["tiles"]!!.jsonArray
        assertEquals(2, tiles.size)
        val button = tiles[0].jsonObject
        assertEquals("17", button["id"]!!.jsonPrimitive.content)
        assertEquals("button", button["kind"]!!.jsonPrimitive.content)
        assertEquals("ext.speaker-muted",
            button["state"]!!.jsonObject["channel"]!!.jsonPrimitive.content)
        val slider = tiles[1].jsonObject
        assertEquals("slider", slider["kind"]!!.jsonPrimitive.content)
        assertEquals("slide", slider["interactions"]!!.jsonArray[0].jsonPrimitive.content)
    }

    @Test
    fun boardsDeltaParses() {
        val frame = fixture("boards.delta.json")
        assertEquals("boards.delta", frame.type)
        val delta = json.decodeFromString(BoardsDelta.serializer(), frame.payloadObject().toString())
        assertEquals(8L, delta.generation)
        assertEquals(2, delta.ops.size)
        assertEquals("tile-set", delta.ops[0].jsonObject["op"]!!.jsonPrimitive.content)
        assertEquals("board-remove", delta.ops[1].jsonObject["op"]!!.jsonPrimitive.content)
    }

    @Test
    fun stateSyncParses() {
        val frame = fixture("state.sync.json")
        val sync = json.decodeFromString(StateSync.serializer(), frame.payloadObject().toString())
        assertEquals("OFF", sync.values["ext.speaker-muted"]!!.jsonPrimitive.content)
        assertEquals(true, sync.values["discord.microphone-muted"]!!.jsonPrimitive.boolean)
        assertEquals(listOf(0.1, 0.42, 0.44), sync.series["ext.si-cpu-usage"])
    }

    @Test
    fun statePatchParses() {
        val frame = fixture("state.patch.json")
        assertEquals("state.patch", frame.type)
        val patch = json.decodeFromString(StatePatch.serializer(), frame.payloadObject().toString())
        assertEquals(3, patch.changes.size)
        assertEquals("ext.si-cpu-usage", patch.changes[1].channel)
        // list channel: assert the content, not just the shape
        assertEquals(listOf("a", "b"), patch.changes[2].value.jsonArray.map { it.jsonPrimitive.content })
    }

    @Test
    fun interactionParses() {
        val frame = fixture("interaction.json")
        assertEquals("interaction", frame.type)
        assertEquals("i9", frame.id)
        val interaction = json.decodeFromString(InteractionPayload.serializer(), frame.payloadObject().toString())
        assertEquals(3L, interaction.board)
        assertEquals(21L, interaction.tile)
        assertEquals("slide", interaction.interaction)
        assertEquals(0.5, interaction.args.value!!, 1e-9)
    }
}
