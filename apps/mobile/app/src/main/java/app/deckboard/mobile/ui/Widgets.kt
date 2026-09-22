//! Widget templates over protocol v2: graph sparkline fed by the server's
//! series ring buffer, knob, list, and the locally rendered clock (the
//! tile announces itself via `params.widget = "clock"`).

package app.deckboard.mobile.ui

import android.content.Context
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.deckboard.mobile.proto.ChannelInfo
import app.deckboard.mobile.proto.Tile
import kotlinx.coroutines.delay
import kotlinx.serialization.json.jsonPrimitive
import kotlin.math.abs
import kotlin.math.atan2
import kotlin.math.ceil
import kotlin.math.cos
import kotlin.math.max
import kotlin.math.min
import kotlin.math.sin

private fun parse(value: Double?): String =
    if (value == null) "" else if (value == ceil(value)) value.toInt().toString() else "%.1f".format(value)

/** Clock visual styles. Tapping the tile cycles to the next entry and
 *  the choice persists per tile id in shared preferences. */
val CLOCK_STYLES = listOf("icon", "big", "analog", "date")

private fun clockText(cal: java.util.Calendar, twelveHour: Boolean): String {
    val minute = cal.get(java.util.Calendar.MINUTE).toString().padStart(2, '0')
    return if (twelveHour) {
        val h = cal.get(java.util.Calendar.HOUR_OF_DAY)
        val hour12 = if (h % 12 == 0) 12 else h % 12
        val ampm = if (cal.get(java.util.Calendar.AM_PM) == java.util.Calendar.AM) "AM" else "PM"
        "$hour12:$minute $ampm"
    } else {
        "${cal.get(java.util.Calendar.HOUR_OF_DAY).toString().padStart(2, '0')}:$minute"
    }
}

private fun hand(center: Offset, angleDeg: Double, len: Float): Offset {
    val a = Math.toRadians(angleDeg)
    return center + Offset((len * sin(a)).toFloat(), (-len * cos(a)).toFloat())
}

/** Native clock. `params.clock_format` ("12h"/"24h") picks the format;
 *  the time itself comes from the device and re-renders right after each
 *  minute boundary, no JS round-trip. Tapping the tile cycles CLOCK_STYLES. */
@Composable
fun ClockTile(
    tile: Tile,
    icon: String,
    iconFamily: FontFamily,
    titleColor: Color,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    val prefs = remember(tile.id) { context.getSharedPreferences("deckboard", Context.MODE_PRIVATE) }
    var style by remember(tile.id) {
        mutableStateOf(prefs.getString("clock_style_${tile.id}", "icon") ?: "icon")
    }
    var now by remember(tile.id) { mutableStateOf(java.util.Calendar.getInstance()) }
    LaunchedEffect(tile.id) {
        while (true) {
            now = java.util.Calendar.getInstance()
            delay(60_000L - (now.get(java.util.Calendar.SECOND) * 1000L + now.get(java.util.Calendar.MILLISECOND)))
        }
    }
    Box(
        modifier
            .fillMaxSize()
            .pointerInput(tile.id) {
                detectTapGestures(onTap = {
                    style = CLOCK_STYLES[(CLOCK_STYLES.indexOf(style) + 1) % CLOCK_STYLES.size]
                    prefs.edit().putString("clock_style_${tile.id}", style).apply()
                })
            },
        contentAlignment = Alignment.Center,
    ) {
        val twelveHour = tile.param("clock_format") == "12h"
        when (style) {
            // huge bare time filling the tile
            "big" -> Text(
                text = clockText(now, twelveHour),
                fontSize = 30.sp,
                fontWeight = FontWeight.Bold,
                color = titleColor,
            )
            // drawn face with hour and minute hands
            "analog" -> Canvas(Modifier.fillMaxSize().padding(12.dp)) {
                val stroke = 4f
                val radius = min(size.width, size.height) / 2f - stroke
                val center = Offset(size.width / 2f, size.height / 2f)
                drawCircle(titleColor, radius = radius, center = center, style = Stroke(stroke))
                repeat(4) { i ->
                    drawLine(
                        titleColor,
                        start = hand(center, i * 90.0, radius * 0.82f),
                        end = hand(center, i * 90.0, radius),
                        strokeWidth = stroke,
                        cap = StrokeCap.Round,
                    )
                }
                val cal = now
                val minuteAngle = cal.get(java.util.Calendar.MINUTE) / 60.0 * 360.0
                val hourAngle = (cal.get(java.util.Calendar.HOUR_OF_DAY) % 12) / 12.0 * 360.0 +
                    cal.get(java.util.Calendar.MINUTE) / 720.0 * 360.0
                drawLine(
                    titleColor,
                    start = center,
                    end = hand(center, hourAngle, radius * 0.5f),
                    strokeWidth = stroke * 1.6f,
                    cap = StrokeCap.Round,
                )
                drawLine(
                    titleColor,
                    start = center,
                    end = hand(center, minuteAngle, radius * 0.78f),
                    strokeWidth = stroke,
                    cap = StrokeCap.Round,
                )
                drawCircle(titleColor, radius = stroke * 1.2f, center = center)
            }
            // time with the weekday and date underneath
            "date" -> Column(horizontalAlignment = Alignment.CenterHorizontally) {
                Text(
                    text = clockText(now, twelveHour),
                    fontSize = 20.sp,
                    fontWeight = FontWeight.Bold,
                    color = titleColor,
                )
                Text(
                    text = java.text.SimpleDateFormat("EEE d.MM", java.util.Locale.getDefault())
                        .format(now.time),
                    fontSize = 11.sp,
                    color = titleColor.copy(alpha = 0.75f),
                )
            }
            // original look: glyph above the time
            else -> Column(horizontalAlignment = Alignment.CenterHorizontally) {
                if (icon.isNotEmpty()) {
                    Text(
                        text = faChar(icon),
                        fontFamily = iconFamily,
                        fontSize = 20.sp,
                        color = titleColor,
                    )
                }
                Text(
                    text = clockText(now, twelveHour),
                    fontSize = 18.sp,
                    fontWeight = FontWeight.Bold,
                    color = titleColor,
                )
            }
        }
    }
}

/** Line chart of the server-side series window. Mirrors the desktop
 * editor's graph tile: bold title over a big current value (with the
 * channel's unit suffix) in the top-left, the chart bleeding to the
 * bottom edges. Title and suffix come from the tile or the welcome
 * catalog (captured by the server from pushed custom values). */
@Composable
fun GraphTile(
    tile: Tile,
    history: SeriesWindow,
    liveText: String?,
    channel: ChannelInfo?,
    titleColor: Color,
    modifier: Modifier = Modifier,
) {
    val lineColor = titleColor.copy(alpha = 0.9f)
    val title = tile.style?.title ?: channel?.title
    val suffix = channel?.suffix.orEmpty()
    Column(
        modifier.fillMaxSize(),
    ) {
        Column(Modifier.fillMaxWidth().padding(start = 8.dp, top = 6.dp, end = 8.dp)) {
            if (!title.isNullOrEmpty()) {
                Text(
                    text = title,
                    fontSize = 12.sp,
                    fontWeight = FontWeight.Bold,
                    color = titleColor,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Text(
                text = (liveText ?: liveFromSeries(history).orEmpty()) + suffix,
                fontSize = 19.sp,
                fontWeight = FontWeight.Bold,
                color = titleColor,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Canvas(Modifier.fillMaxWidth().fillMaxHeight()) {
            // The server keeps a 120-point window; that is denser than a
            // tile can show, so bucket-average down to MAX_DRAWN_POINTS -
            // same shape, calmer line.
            val points = downsample(history.points, MAX_DRAWN_POINTS)
            if (points.size < 2) return@Canvas
            // normalize around the window's average so the ordinary level
            // sits at mid-height: a strong machine idles near a few percent
            // and a fixed 0..100 scale would pin the whole curve to the
            // floor. The span has an absolute and relative floor so a quiet
            // series stays calm instead of amplifying noise to full height.
            val avg = points.sum() / points.size
            val dev = maxOf(points.max() - avg, avg - points.min())
            val half = maxOf(dev, 0.2 * abs(avg), 5.0)
            val minV = avg - half
            val span = 2.0 * half
            val stepX = size.width / (points.size - 1)
            val line = Path()
            points.forEachIndexed { i, v ->
                val x = i * stepX
                val y = size.height - ((v - minV) / span).toFloat().coerceIn(0f, 1f) * size.height
                if (i == 0) line.moveTo(x, y) else line.lineTo(x, y)
            }
            // wash the area under the curve with a lighter tone of the
            // line color so the filled side reads as background, not data
            val area = Path().apply {
                addPath(line)
                lineTo(size.width, size.height)
                lineTo(0f, size.height)
                close()
            }
            drawPath(area, lineColor.copy(alpha = 0.35f))
            drawPath(line, lineColor, style = Stroke(width = 3f, cap = StrokeCap.Round))
        }
    }
}

/** Newest series point as text - covers the gap between a (re)connect
 *  (only `state.sync` arrived, no patch yet) and the next producer push. */
private fun liveFromSeries(history: SeriesWindow): String? {
    val last = history.points.lastOrNull() ?: return null
    return if (last == kotlin.math.floor(last) && !last.isInfinite()) {
        last.toLong().toString()
    } else {
        last.toString()
    }
}

/** Upper bound on points drawn per chart; larger windows are averaged
 *  per bucket so small tiles stay readable. */
private const val MAX_DRAWN_POINTS = 40

/** Bucket-average [history] down to at most [max] points (keeps shape,
 *  drops jitter). A no-op when the window already fits. */
internal fun downsample(history: List<Double>, max: Int): List<Double> {
    if (history.size <= max) return history
    val bucket = history.size.toDouble() / max
    return List(max) { i ->
        val from = kotlin.math.floor(i * bucket).toInt()
        val to = minOf(kotlin.math.ceil((i + 1) * bucket).toInt(), history.size)
        history.subList(from, to).average()
    }
}

/**
 * Circular control: drag around the center to set a 0..1 value, sent via
 * the same slide interaction as slider tiles.
 */
@Composable
fun KnobTile(
    tile: Tile,
    baseColor: Color,
    iconColor: Color,
    titleColor: Color,
    onSlider: (Float) -> Unit,
    modifier: Modifier = Modifier,
) {
    var value by remember(tile.id) { mutableFloatStateOf(0.5f) }
    val arcColor = tile.style?.color2?.let { hex(it, titleColor) } ?: titleColor

    Box(
        modifier
            .fillMaxSize()
            .pointerInput(tile.id) {
                detectDragGestures(
                    onDrag = { change, _ ->
                        change.consume()
                        val center = Offset(size.width / 2f, size.height / 2f)
                        val pos = change.position - center
                        // screen angles are clockwise-down; convert so that
                        // value grows clockwise from the top (like a dial)
                        var angle = Math.toDegrees(atan2(pos.x.toDouble(), -pos.y.toDouble()))
                        if (angle < 0) angle += 360.0
                        val radius = max(1.0, kotlin.math.hypot(pos.x.toDouble(), pos.y.toDouble()))
                        val ring = radius.coerceAtMost(size.width / 2.0)
                        val dead = ring * 0.25
                        val raw = ((angle - 135.0 + 360.0) % 360.0) / 270.0
                        val clamped = raw.coerceIn(0.0, 1.0).toFloat()
                        val scaled = if (radius < dead) value else clamped
                        if (scaled != value) {
                            value = scaled
                            onSlider(value)
                        }
                    },
                )
            },
        contentAlignment = Alignment.Center,
    ) {
        Canvas(Modifier.fillMaxSize().padding(10.dp)) {
            val stroke = 10f
            val inset = stroke
            val arcDim = titleColor.copy(alpha = 0.25f)
            // background ring: 270 degrees starting bottom-left
            drawArc(
                color = arcDim,
                startAngle = 135f,
                sweepAngle = 270f,
                useCenter = false,
                style = Stroke(stroke),
            )
            drawArc(
                color = arcColor,
                startAngle = 135f,
                sweepAngle = 270f * value,
                useCenter = false,
                style = Stroke(stroke),
            )
            // pointer dot at the current angle
            val angle = Math.toRadians((135.0 + 270.0 * value).toDouble())
            val radius = min(size.width, size.height) / 2f - inset
            drawCircle(
                color = iconColor,
                radius = stroke,
                center = Offset(
                    center.x + radius * cos(angle).toFloat(),
                    center.y + radius * sin(angle).toFloat(),
                ),
            )
        }
        Text(
            text = parse(value.toDouble()),
            fontSize = 16.sp,
            fontWeight = FontWeight.Bold,
            color = titleColor,
        )
    }
}

/**
 * Scrollable option list. Items come from the manifest (`params.items`)
 * or from the live list value when the extension pushes an array. Tapping
 * an item selects it and fires the tile's tap.
 */
@Composable
fun ListTile(
    tile: Tile,
    items: TileItems,
    titleColor: Color,
    onPress: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var selected by remember(tile.id) { androidx.compose.runtime.mutableStateOf(-1) }
    Column(
        modifier
            .fillMaxSize()
            .padding(4.dp)
            .clip(RoundedCornerShape(6.dp))
            .background(titleColor.copy(alpha = 0.08f)),
    ) {
        if (!tile.style?.title.isNullOrEmpty()) {
            Text(
                text = tile.style?.title.orEmpty(),
                fontSize = 11.sp,
                color = titleColor,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(start = 6.dp, top = 4.dp),
            )
        }
        LazyColumn(Modifier.fillMaxSize()) {
            itemsIndexed(items.values) { i, item ->
                val highlight = i == selected
                Row(
                    Modifier
                        .fillMaxWidth()
                        .clickable {
                            selected = i
                            onPress()
                        }
                        .background(if (highlight) titleColor.copy(alpha = 0.25f) else Color.Transparent)
                        .padding(horizontal = 6.dp, vertical = 3.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        text = item,
                        fontSize = 12.sp,
                        color = titleColor,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
        }
    }
}

/** Items for a list tile: manifest items first, else a pushed array value. */
fun listItems(tile: Tile, live: kotlinx.serialization.json.JsonElement?): List<String> {
    val fromManifest = (tile.params as? kotlinx.serialization.json.JsonObject)
        ?.get("items")
        ?.let { el ->
            (el as? kotlinx.serialization.json.JsonArray)?.mapNotNull { item ->
                when (item) {
                    is kotlinx.serialization.json.JsonPrimitive -> item.content
                    is kotlinx.serialization.json.JsonObject ->
                        item["label"]?.let { runCatching { it.jsonPrimitive.content }.getOrNull() }
                    else -> null
                }
            }
        }
    if (!fromManifest.isNullOrEmpty()) return fromManifest

    return when (live) {
        is kotlinx.serialization.json.JsonArray -> live.mapNotNull {
            runCatching { it.jsonPrimitive.content }.getOrNull()
        }
        else -> emptyList()
    }
}
