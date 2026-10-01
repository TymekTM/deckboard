package app.pulpit.mobile.ui

import app.pulpit.mobile.proto.BoardOp
import app.pulpit.mobile.proto.BoardsSync
import app.pulpit.mobile.proto.Frame
import app.pulpit.mobile.proto.Tile
import app.pulpit.mobile.proto.V2
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Tile field handling over the golden fixtures
 * (`crates/proto/tests/fixtures`, wired in as test resources): the tiles
 * the wire actually sends must survive template choice, press-mode and
 * style/state parsing. Companion to ProtoFixturesTest, which covers the
 * envelope models.
 */
class TileFieldHandlingTest {

    private val json = Json { ignoreUnknownKeys = true; isLenient = true }

    private fun fixturePayload(name: String): String {
        val loader = checkNotNull(javaClass.classLoader) { "no test class loader" }
        val stream = loader.getResourceAsStream(name)
            ?: error("fixture $name not on the test classpath")
        val frame = json.decodeFromString(Frame.serializer(), stream.readBytes().decodeToString())
        return frame.payload.toString()
    }

    private fun fixtureTiles(): List<Tile> =
        json.decodeFromString(BoardsSync.serializer(), fixturePayload("boards.sync.json"))
            .boards[0].tiles

    private fun tile(wire: String): Tile = json.decodeFromString(Tile.serializer(), wire)

    @Test
    fun fixtureButtonTileKeepsItsFields() {
        val button = fixtureTiles()[0]
        assertEquals(V2.KIND_BUTTON, button.kind)
        // state binding: channel + shape must both survive the parse
        assertEquals("ext.speaker-muted", button.state!!.channel)
        assertEquals(V2.SHAPE_SCALAR, button.state!!.shape)
        // style fields the face renders from
        assertEquals("#F5AB35", button.style!!.color)
        assertEquals("\uf026", button.style!!.icon)
        assertEquals("Mute", button.style!!.title)
        // a plain button declares the tap gesture only
        assertFalse(button.interacts(V2.INT_PRESS_START))
        assertFalse(button.interacts(V2.INT_PRESS_END))
        assertTrue(button.interacts(V2.INT_TAP))
    }

    @Test
    fun fixtureSliderTileDeclaresOnlySlide() {
        val slider = fixtureTiles()[1]
        assertEquals(V2.KIND_SLIDER, slider.kind)
        assertEquals(2, slider.h)
        assertTrue(slider.interacts(V2.INT_SLIDE))
        assertFalse(slider.interacts(V2.INT_PRESS_START))
    }

    @Test
    fun deltaTileSetCarriesPressModes() {
        // the delta fixture rewrites tile 17 as a hold-to-repeat key that
        // still declares its plain tap
        val ops = json.decodeFromString(
            app.pulpit.mobile.proto.BoardsDelta.serializer(),
            fixturePayload("boards.delta.json"),
        ).ops.mapNotNull { BoardOp.from(it, json) }
        val tileSet = ops[0] as BoardOp.TileSet
        assertTrue(tileSet.tile.interacts(V2.INT_PRESS_START))
        assertTrue(tileSet.tile.interacts(V2.INT_PRESS_END))
        assertTrue(tileSet.tile.interacts(V2.INT_TAP))
    }

    @Test
    fun templateFollowsKindAndDegradesToButton() {
        assertEquals("button", templateFor(fixtureTiles()[0]))
        assertEquals("slider", templateFor(fixtureTiles()[1]))
        assertEquals("knob", templateFor(tile("""{"id":1,"kind":"knob"}""")))
        assertEquals("graph", templateFor(tile("""{"id":1,"kind":"graph"}""")))
        assertEquals("list", templateFor(tile("""{"id":1,"kind":"list"}""")))
        assertEquals("toggle", templateFor(tile("""{"id":1,"kind":"toggle"}""")))
        // unknown kinds degrade to the button template, never fail
        assertEquals("button", templateFor(tile("""{"id":1,"kind":"holo-deck"}""")))
    }

    @Test
    fun clockWidgetHintOutranksTheWireKind() {
        val clock = tile("""{"id":1,"kind":"toggle","params":{"widget":"clock"}}""")
        assertEquals("clock", templateFor(clock))
        assertEquals("clock", clock.widgetHint())
    }
}
