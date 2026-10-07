package app.pulpit.mobile.ui

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.pulpit.mobile.proto.Tile
import java.text.SimpleDateFormat
import java.util.Calendar
import java.util.Locale
import java.util.TimeZone
import kotlinx.coroutines.delay
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull

/** Pure extrapolation for countdown timer remaining milliseconds. */
fun timerExtrapolateRemaining(
    durationMs: Long,
    elapsedMs: Long,
    startedAtMs: Long,
    running: Boolean,
    finished: Boolean,
    nowMs: Long,
): Long {
    if (finished) return 0L
    val totalElapsed = if (running) {
        elapsedMs + kotlin.math.max(0L, nowMs - startedAtMs)
    } else {
        elapsedMs
    }
    return kotlin.math.max(0L, durationMs - totalElapsed)
}

/**
 * The tile's compact state object, keeping the last one seen while a
 * legacy stock client is connected: the 1 Hz lane then pushes a plain
 * formatted label string under the same channel (the stock client can
 * only show text), and falling back to tile params on every one of those
 * pushes would make the timer flicker between views.
 */
@Composable
fun rememberToolState(live: JsonElement?): JsonObject? {
    var last by remember { mutableStateOf<JsonObject?>(null) }
    (live as? JsonObject)?.let { last = it }
    return last
}

/** Pure extrapolation for stopwatch elapsed milliseconds. */
fun stopwatchExtrapolateElapsed(
    elapsedMs: Long,
    startedAtMs: Long,
    running: Boolean,
    nowMs: Long,
): Long {
    return if (running) {
        elapsedMs + kotlin.math.max(0L, nowMs - startedAtMs)
    } else {
        elapsedMs
    }
}

/** Formats milliseconds as mm:ss or hh:mm:ss. */
fun formatMmSs(ms: Long): String {
    val totalSecs = (ms + 999) / 1000
    val hours = totalSecs / 3600
    val mins = (totalSecs % 3600) / 60
    val secs = totalSecs % 60
    return if (hours > 0) {
        String.format(Locale.ROOT, "%02d:%02d:%02d", hours, mins, secs)
    } else {
        String.format(Locale.ROOT, "%02d:%02d", mins, secs)
    }
}

/** Parses duration string (e.g. "05:00", "5:00", "300") into milliseconds. */
fun parseDurationString(str: String?): Long {
    val s = str?.trim().orEmpty()
    if (s.isEmpty()) return 300_000L
    s.toLongOrNull()?.let { return it * 1000L }
    val parts = s.split(":")
    return when (parts.size) {
        2 -> {
            val m = parts[0].trim().toLongOrNull() ?: 0L
            val sec = parts[1].trim().toLongOrNull() ?: 0L
            (m * 60 + sec) * 1000L
        }
        3 -> {
            val h = parts[0].trim().toLongOrNull() ?: 0L
            val m = parts[1].trim().toLongOrNull() ?: 0L
            val sec = parts[2].trim().toLongOrNull() ?: 0L
            (h * 3600 + m * 60 + sec) * 1000L
        }
        else -> 300_000L
    }
}

/** Native clock tile (tool-clock). Rendered client-side from the device clock. */
@Composable
fun ToolClockTile(
    tile: Tile,
    titleColor: Color,
    modifier: Modifier = Modifier,
) {
    val twelveHour = tile.param("format") == "12h"
    val showSeconds = tile.param("seconds") in listOf("yes", "true", "1")
    val showDate = tile.param("date") in listOf("yes", "true", "1")
    val tzId = tile.param("timezone")

    var now by remember { mutableStateOf(Calendar.getInstance()) }

    LaunchedEffect(showSeconds, tzId) {
        while (true) {
            val cal = if (!tzId.isNullOrBlank()) {
                Calendar.getInstance(TimeZone.getTimeZone(tzId))
            } else {
                Calendar.getInstance()
            }
            now = cal
            if (showSeconds) {
                delay(1000L - (cal.get(Calendar.MILLISECOND)))
            } else {
                val sec = cal.get(Calendar.SECOND)
                val ms = cal.get(Calendar.MILLISECOND)
                delay(60_000L - (sec * 1000L + ms))
            }
        }
    }

    val timePattern = when {
        twelveHour && showSeconds -> "hh:mm:ss a"
        twelveHour -> "hh:mm a"
        showSeconds -> "HH:mm:ss"
        else -> "HH:mm"
    }

    val timeFormat = remember(timePattern, tzId) {
        SimpleDateFormat(timePattern, Locale.getDefault()).apply {
            if (!tzId.isNullOrBlank()) timeZone = TimeZone.getTimeZone(tzId)
        }
    }

    val dateFormat = remember(tzId) {
        SimpleDateFormat("EEE, d MMM", Locale.getDefault()).apply {
            if (!tzId.isNullOrBlank()) timeZone = TimeZone.getTimeZone(tzId)
        }
    }

    Box(
        modifier = modifier.fillMaxSize().padding(8.dp),
        contentAlignment = Alignment.Center,
    ) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Text(
                text = timeFormat.format(now.time),
                color = titleColor,
                fontSize = if (showSeconds) 20.sp else 26.sp,
                fontWeight = FontWeight.Bold,
                textAlign = TextAlign.Center,
            )
            if (showDate) {
                Spacer(Modifier.height(4.dp))
                Text(
                    text = dateFormat.format(now.time),
                    color = titleColor.copy(alpha = 0.8f),
                    fontSize = 12.sp,
                    textAlign = TextAlign.Center,
                )
            }
        }
    }
}

/** Native countdown timer tile (tool-timer). */
@Composable
fun ToolTimerTile(
    tile: Tile,
    live: JsonElement?,
    titleColor: Color,
    onPress: () -> Unit,
    onGesture: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val obj = rememberToolState(live)
    val running = obj?.get("running")?.jsonPrimitive?.booleanOrNull ?: false
    val finished = obj?.get("finished")?.jsonPrimitive?.booleanOrNull ?: false
    val startedAtMs = obj?.get("startedAtMs")?.jsonPrimitive?.longOrNull ?: 0L
    val elapsedMs = obj?.get("elapsedMs")?.jsonPrimitive?.longOrNull ?: 0L
    val durationMs = obj?.get("durationMs")?.jsonPrimitive?.longOrNull
        ?: parseDurationString(tile.param("duration"))

    var nowTick by remember { mutableLongStateOf(System.currentTimeMillis()) }

    LaunchedEffect(running) {
        if (running) {
            while (true) {
                nowTick = System.currentTimeMillis()
                delay(200L)
            }
        } else {
            nowTick = System.currentTimeMillis()
        }
    }

    val remaining = timerExtrapolateRemaining(
        durationMs = durationMs,
        elapsedMs = elapsedMs,
        startedAtMs = startedAtMs,
        running = running,
        finished = finished,
        nowMs = nowTick,
    )

    val isFinished = finished || (remaining == 0L && durationMs > 0 && (running || elapsedMs >= durationMs))

    // Flashing effect when timer is finished
    val infiniteTransition = rememberInfiniteTransition(label = "timerFlash")
    val flashAlpha by infiniteTransition.animateFloat(
        initialValue = 0f,
        targetValue = 0.85f,
        animationSpec = infiniteRepeatable(
            animation = tween(400),
            repeatMode = RepeatMode.Reverse,
        ),
        label = "flashAlpha",
    )

    Box(
        modifier = modifier
            .fillMaxSize()
            .pointerInput(tile.id) {
                detectTapGestures(
                    onTap = { onPress() },
                    onDoubleTap = { onGesture("double-tap") },
                    onLongPress = { onGesture("long-press") },
                )
            },
        contentAlignment = Alignment.Center,
    ) {
        if (isFinished) {
            Box(
                Modifier
                    .fillMaxSize()
                    .background(Color(0xFFE74C3C).copy(alpha = flashAlpha)),
            )
        }

        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            modifier = Modifier.padding(6.dp),
        ) {
            Text(
                text = formatMmSs(remaining),
                color = titleColor,
                fontSize = 24.sp,
                fontWeight = FontWeight.Bold,
                textAlign = TextAlign.Center,
            )
            Spacer(Modifier.height(4.dp))
            val statusLabel = when {
                isFinished -> "KONIEC!"
                running -> "Działa"
                elapsedMs > 0 -> "Pauza"
                else -> "Gotowy"
            }
            Text(
                text = statusLabel,
                color = titleColor.copy(alpha = 0.75f),
                fontSize = 11.sp,
                textAlign = TextAlign.Center,
            )
        }
    }
}

/** Native stopwatch tile (tool-stopwatch). */
@Composable
fun ToolStopwatchTile(
    tile: Tile,
    live: JsonElement?,
    titleColor: Color,
    onPress: () -> Unit,
    onGesture: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val obj = rememberToolState(live)
    val running = obj?.get("running")?.jsonPrimitive?.booleanOrNull ?: false
    val startedAtMs = obj?.get("startedAtMs")?.jsonPrimitive?.longOrNull ?: 0L
    val elapsedMs = obj?.get("elapsedMs")?.jsonPrimitive?.longOrNull ?: 0L

    var nowTick by remember { mutableLongStateOf(System.currentTimeMillis()) }

    LaunchedEffect(running) {
        if (running) {
            while (true) {
                nowTick = System.currentTimeMillis()
                delay(200L)
            }
        } else {
            nowTick = System.currentTimeMillis()
        }
    }

    val elapsed = stopwatchExtrapolateElapsed(
        elapsedMs = elapsedMs,
        startedAtMs = startedAtMs,
        running = running,
        nowMs = nowTick,
    )

    Box(
        modifier = modifier
            .fillMaxSize()
            .pointerInput(tile.id) {
                detectTapGestures(
                    onTap = { onPress() },
                    onDoubleTap = { onGesture("double-tap") },
                    onLongPress = { onGesture("long-press") },
                )
            },
        contentAlignment = Alignment.Center,
    ) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            modifier = Modifier.padding(6.dp),
        ) {
            Text(
                text = formatMmSs(elapsed),
                color = titleColor,
                fontSize = 24.sp,
                fontWeight = FontWeight.Bold,
                textAlign = TextAlign.Center,
            )
            Spacer(Modifier.height(4.dp))
            val statusLabel = if (running) "Działa" else if (elapsed > 0) "Pauza" else "00:00"
            Text(
                text = statusLabel,
                color = titleColor.copy(alpha = 0.75f),
                fontSize = 11.sp,
                textAlign = TextAlign.Center,
            )
        }
    }
}

/** Native counter tile (tool-counter). */
@Composable
fun ToolCounterTile(
    tile: Tile,
    live: JsonElement?,
    titleColor: Color,
    onPress: () -> Unit,
    onGesture: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val obj = rememberToolState(live)
    val startVal = tile.param("start_value")?.toLongOrNull() ?: 0L
    val count = obj?.get("count")?.jsonPrimitive?.longOrNull ?: startVal
    val label = tile.param("label")?.ifBlank { null } ?: "Licznik"

    Box(
        modifier = modifier
            .fillMaxSize()
            .pointerInput(tile.id) {
                detectTapGestures(
                    onTap = { onPress() },
                    onDoubleTap = { onGesture("double-tap") },
                    onLongPress = { onGesture("long-press") },
                )
            },
        contentAlignment = Alignment.Center,
    ) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            modifier = Modifier.padding(6.dp),
        ) {
            Text(
                text = label,
                color = titleColor.copy(alpha = 0.8f),
                fontSize = 12.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                textAlign = TextAlign.Center,
            )
            Spacer(Modifier.height(2.dp))
            Text(
                text = count.toString(),
                color = titleColor,
                fontSize = 32.sp,
                fontWeight = FontWeight.Bold,
                textAlign = TextAlign.Center,
            )
        }
    }
}
