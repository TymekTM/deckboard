//! MOB-07: the bitmap cache is byte-budgeted - oldest entries evict
//! once the running weight crosses the cap, and eviction reports back.

package app.pulpit.mobile.state

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class BoundedLruTest {
    @Test
    fun putsUnderBudgetAreKept() {
        val lru = BoundedLru<String, Int>(maxWeight = 10) { it.toLong() }
        lru.put("a", 3)
        lru.put("b", 4)
        assertEquals(3, lru["a"])
        assertEquals(4, lru["b"])
        assertEquals(mapOf("a" to 3, "b" to 4), lru.snapshot())
    }

    @Test
    fun crossingTheBudgetEvictsOldestFirst() {
        val evicted = mutableListOf<String>()
        val lru = BoundedLru<String, Int>(maxWeight = 10) { it.toLong() }
        lru.onEvict = { evicted.add(it) }
        lru.put("a", 4)
        lru.put("b", 4)
        lru.put("c", 4) // weight 12 > 10 -> "a" evicts
        assertEquals(listOf("a"), evicted)
        assertNull(lru["a"])
        assertEquals(4, lru["b"])
        assertEquals(4, lru["c"])
    }

    @Test
    fun rePuttingAnExistingKeyReplacesItsWeight() {
        val lru = BoundedLru<String, Int>(maxWeight = 10) { it.toLong() }
        lru.put("a", 5)
        lru.put("a", 2) // replaced, not doubled
        lru.put("b", 7) // 2 + 7 = 9 <= 10, nothing evicts
        assertEquals(2, lru["a"])
        assertEquals(7, lru["b"])
    }

    @Test
    fun anEntryHeavierThanTheWholeBudgetIsDroppedImmediately() {
        val evicted = mutableListOf<String>()
        val lru = BoundedLru<String, Int>(maxWeight = 5) { it.toLong() }
        lru.onEvict = { evicted.add(it) }
        lru.put("huge", 100)
        assertTrue(evicted.contains("huge"))
        assertFalse("huge" in lru)
        assertTrue(lru.snapshot().isEmpty())
    }

    @Test
    fun clearDropsEverythingWithoutEvictionCallbacks() {
        var evictions = 0
        val lru = BoundedLru<String, Int>(maxWeight = 100) { it.toLong() }
        lru.onEvict = { evictions++ }
        lru.put("a", 1)
        lru.put("b", 2)
        lru.clear()
        assertTrue(lru.snapshot().isEmpty())
        assertEquals(0, evictions)
    }

    @Test
    fun snapshotIsACopy() {
        val lru = BoundedLru<String, Int>(maxWeight = 100) { it.toLong() }
        lru.put("a", 1)
        val snap = lru.snapshot()
        lru.put("b", 2)
        assertEquals(1, snap.size)
        assertEquals(2, lru.snapshot().size)
    }
}
