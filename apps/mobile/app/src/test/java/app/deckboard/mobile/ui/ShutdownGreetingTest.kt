package app.deckboard.mobile.ui

import org.junit.Assert.assertEquals
import org.junit.Test

/** The shutdown standby screen greets by the tablet's local hour: night
 *  says goodnight, everything else goodbye. */
class ShutdownGreetingTest {

    @Test
    fun night_hours_say_goodnight() {
        for (hour in listOf(21, 22, 23, 0, 3, 5)) {
            assertEquals("Goodnight", shutdownGreeting(hour))
        }
    }

    @Test
    fun daytime_hours_say_goodbye() {
        for (hour in listOf(6, 9, 12, 17, 20)) {
            assertEquals("Goodbye", shutdownGreeting(hour))
        }
    }
}
