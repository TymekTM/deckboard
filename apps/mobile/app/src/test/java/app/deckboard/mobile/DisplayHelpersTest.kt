package app.deckboard.mobile.net

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Pure display helpers the board grid uses on pushed channel values. */
class DisplayHelpersTest {

    private val json = Json

    @Test
    fun displayTextRendersScalarsAndValueObjects() {
        assertNull(displayText(null))
        assertEquals("ON", displayText(JsonPrimitive("ON")))
        assertEquals("19.6", displayText(json.parseToJsonElement("19.6")))
        assertEquals("45.2%", displayText(json.parseToJsonElement("""{"value":"45.2","suffix":"%"}""")))
        // a non-primitive without a value field has no text form
        assertNull(displayText(json.parseToJsonElement("""{"a":1}""")))
    }

    @Test
    fun isActiveValueCoversTheLegacyEncodings() {
        assertTrue(isActiveValue(JsonPrimitive("ON")))
        assertTrue(isActiveValue(JsonPrimitive("1")))
        assertTrue(isActiveValue(json.parseToJsonElement("true")))
        assertFalse(isActiveValue(JsonPrimitive("OFF")))
        assertFalse(isActiveValue(json.parseToJsonElement("false")))
        assertFalse(isActiveValue(json.parseToJsonElement("""{"value":"ON"}""")))
        assertFalse(isActiveValue(null))
    }

    @Test
    fun numericValueUnwrapsValueObjects() {
        assertEquals(3.5, numericValue(json.parseToJsonElement("3.5"))!!, 1e-9)
        assertEquals(3.5, numericValue(json.parseToJsonElement("""{"value":3.5}"""))!!, 1e-9)
        assertNull(numericValue(JsonPrimitive("idle")))
        assertNull(numericValue(null))
    }
}
