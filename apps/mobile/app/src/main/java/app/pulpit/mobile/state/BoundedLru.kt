//! Byte-budgeted LRU for decoded asset bitmaps (round 4, MOB-07): a
//! deck that cycles many boards with distinct backgrounds used to keep
//! every bitmap it ever decoded for the whole process lifetime on a
//! 1 GB device. Entries evict oldest-first whenever the running weight
//! crosses the budget; a single entry heavier than the whole budget is
//! dropped immediately (the memory-safe choice).

package app.pulpit.mobile.state

/** Insertion-ordered map with a weight budget. Pure bookkeeping, unit
 *  tested with plain values; the ViewModel uses it for
 *  `hash -> ImageBitmap` with ARGB byte weights. */
class BoundedLru<K, V>(private val maxWeight: Long, private val weightOf: (V) -> Long) {

    private val map = LinkedHashMap<K, V>()
    private var weight = 0L

    /** Invoked for every entry evicted by [put] (not by [clear]). */
    var onEvict: ((K) -> Unit)? = null

    operator fun contains(key: K): Boolean = map.containsKey(key)

    operator fun get(key: K): V? = map[key]

    fun put(key: K, value: V) {
        map.remove(key)?.let { weight -= weightOf(it) }
        map[key] = value
        weight += weightOf(value)
        trim()
    }

    /** Oldest-first eviction down to the budget. */
    private fun trim() {
        val it = map.entries.iterator()
        while (weight > maxWeight && it.hasNext()) {
            val (key, value) = it.next()
            weight -= weightOf(value)
            it.remove()
            onEvict?.invoke(key)
        }
    }

    fun clear() {
        map.clear()
        weight = 0L
    }

    /** An immutable copy for the Compose state flow; the LRU itself
     *  stays confined to the main thread. */
    fun snapshot(): Map<K, V> = LinkedHashMap(map)
}
