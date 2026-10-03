//! Plog tests: the file-line shape and the rotation selection (pure
//! parts only - no Android runtime here).

package app.pulpit.mobile

import app.pulpit.mobile.state.Plog
import org.junit.Assert.assertEquals
import org.junit.Test

class PlogTest {
    @Test
    fun lineIsUtcTaggedAndLevelled() {
        // 2026-10-03 14:21:53 UTC = 1791037313000 ms (fixed instant, not
        // local time: the deck may sit in any timezone)
        val line = Plog.formatLine(1791037313000L, "I", "PulpitViewModel", "reconnect attempt 2")
        assertEquals("2026-10-03T14:21:53Z I PulpitViewModel reconnect attempt 2", line)
    }

    @Test
    fun activeFileNameIsDated() {
        val cal = java.util.Calendar.getInstance(
            java.util.TimeZone.getTimeZone("UTC"),
        ).apply { timeInMillis = 1791037313000L }
        assertEquals("2026-10-03.log", Plog.fileName(cal.time))
    }

    @Test
    fun rotationKeepsTheNewestNames() {
        // names sort chronologically; frozen overflow files ("-<millis>")
        // sort next to their day and survive as part of the newest set
        val names = listOf(
            "2026-10-01.log",
            "2026-10-03.log",
            "2026-09-28.log",
            "2026-10-02-1790000000000.log",
            "2026-10-02.log",
        )
        assertEquals(
            // lexicographic: `-` sorts before `.`, so the frozen copy of
            // the day comes right before the day's active file
            listOf("2026-10-02-1790000000000.log", "2026-10-02.log", "2026-10-03.log"),
            Plog.keepNames(names, 3),
        )
    }
}
