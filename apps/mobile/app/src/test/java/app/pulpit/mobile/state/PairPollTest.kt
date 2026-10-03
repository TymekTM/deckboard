//! MOB-16: the pair-decision poll must stop when the user cancels -
//! exits on the decision word, on stillWaiting flipping false, on the
//! deadline, and on job cancellation.

package app.pulpit.mobile.state

import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PairPollTest {
    @Test
    fun returnsTheDecisionWord() = runBlocking {
        var polls = 0
        val decision = pollPairDecisionLoop(
            deadlineMs = Long.MAX_VALUE,
            now = { 0L },
            sleep = {},
            stillWaiting = { true },
            poll = { polls++; if (polls < 2) "pending" else "approved" },
        )
        assertEquals("approved", decision)
        assertEquals(2, polls)
    }

    @Test
    fun stopsPollingOnceNoLongerWaiting() = runBlocking {
        // the user pressed "Przerwij": the loop must exit before the
        // next poll, not keep waking the radio until the TTL
        var polls = 0
        val decision = pollPairDecisionLoop(
            deadlineMs = Long.MAX_VALUE,
            now = { 0L },
            sleep = {},
            stillWaiting = { false },
            poll = { polls++; "pending" },
        )
        assertNull(decision)
        assertEquals(0, polls)
    }

    @Test
    fun transientPollErrorsKeepLooping() = runBlocking {
        var calls = 0
        val decision = pollPairDecisionLoop(
            deadlineMs = Long.MAX_VALUE,
            now = { 0L },
            sleep = {},
            stillWaiting = { true },
            poll = {
                calls++
                if (calls == 1) throw java.io.IOException("wifi blip") else "rejected"
            },
        )
        assertEquals("rejected", decision)
        assertEquals(2, calls)
    }

    @Test
    fun runsToTheDeadlineAndGivesUp() = runBlocking {
        var tick = 0L
        var polls = 0
        val decision = pollPairDecisionLoop(
            deadlineMs = 3,
            now = { tick },
            sleep = { tick++ },
            stillWaiting = { true },
            poll = { polls++; "pending" },
        )
        assertNull(decision)
        // windows at now = 0, 1, 2 fit before the deadline
        assertEquals(3, polls)
    }

    @Test
    fun jobCancellationEndsTheLoop() = runBlocking {
        // cancelPairRequest cancels the poll job; the cancellation must
        // propagate through the sleep instead of hanging the loop
        val job = launch {
            pollPairDecisionLoop(
                deadlineMs = Long.MAX_VALUE,
                now = { 0L },
                sleep = { delay(60_000) },
                stillWaiting = { true },
                poll = { "pending" },
            )
        }
        job.cancel()
        job.join()
    }
}
