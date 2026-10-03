//! Widget templates over protocol v2: graph sparkline fed by the server's
//! series ring buffer, knob, list, and the locally rendered clock (the
//! tile announces itself via `params.widget = "clock"`).

package app.pulpit.mobile.ui

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
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.pulpit.mobile.proto.ChannelInfo
import app.pulpit.mobile.proto.Tile
import kotlinx.coroutines.delay
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonPrimitive
import kotlin.math.atan2
import kotlin.math.ceil
import kotlin.math.cos
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt
import kotlin.math.sin

private fun parse(value: Double?): String =
    if (value == null) "" else if (value == ceil(value)) value.toInt().toString() else "%.1f".format(value)

/** Static busy ring, shared by the board overlay and the connect
 *  screen: deck tablets often run with animator scales off, which
 *  freezes an indeterminate spinner into an invisible dot. A fixed
 *  300-degree arc reads as "busy" on every device (round 4, MOB-04). */
@Composable
fun RingSpinner(modifier: Modifier = Modifier) {
    Canvas(modifier.size(34.dp)) {
        drawArc(
            color = Color.White,
            startAngle = -90f,
            sweepAngle = 300f,
            useCenter = false,
            style = Stroke(width = 6f, cap = StrokeCap.Round),
        )
    }
}

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
    val prefs = remember(tile.id) { context.getSharedPreferences("pulpit", Context.MODE_PRIVATE) }
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
            // Normalize to the window's own min..max with a 5% pad - the
            // exact rule of the desktop's sparkline (TileCell.vue
            // sparkPoints: min, max, span = max - min || 1, y = 95 - t *
            // 90), so both surfaces draw the same amplitude from the
            // same series: a machine idling at 2-4% spans the full tile
            // on both, instead of a fraction of it here.
            val minV = points.min()
            val span = (points.max() - minV).takeIf { it != 0.0 } ?: 1.0
            val stepX = size.width / (points.size - 1)
            val line = Path()
            points.forEachIndexed { i, v ->
                val x = i * stepX
                val y = size.height - (sparkY(v, minV, span) * size.height).toFloat()
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

/** Sparkline y fraction (0 = top of the tile, 1 = bottom) over the
 *  window's min..max with the desktop's 5% padding - the exact rule of
 *  TileCell.vue's sparkPoints (`95 - t * 90` in percent space), kept
 *  here as the single shared formula (MOB-08). */
internal fun sparkY(v: Double, min: Double, span: Double): Double =
    0.95 - ((v - min) / span) * 0.90

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
    liveValue: Double?,
    onSlider: (Float) -> Unit,
    modifier: Modifier = Modifier,
) {
    // one drag protocol with the slider template (MOB-13): see
    // SlideDragController for the live-echo and convergence policy
    val slide = remember(tile.id) { SlideDragController() }
    // see ButtonTile (Tile.kt): the drag block outlives a live tile edit
    val live by rememberUpdatedState(liveValue)
    val sendSlide by rememberUpdatedState(onSlider)
    val value = slide.current(liveValue)
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
                        val cur = slide.current(live)
                        val raw = ((angle - 135.0 + 360.0) % 360.0) / 270.0
                        val clamped = raw.coerceIn(0.0, 1.0).toFloat()
                        val scaled = if (radius < dead) cur else clamped
                        if (scaled != cur) {
                            slide.move(scaled, send = sendSlide)
                        }
                    },
                    onDragEnd = {
                        // converge: the last sampled value always reaches
                        // the server, throttling only smooths the path
                        slide.end(live, send = sendSlide)
                    },
                    onDragCancel = {
                        // a cancelled drag still commits its last sampled
                        // position (like the desktop), then follows live
                        slide.cancel(live, send = sendSlide)
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

// ---- ai-dev status tiles (agents, plan limits) ------------------------------
// The producer pushes a `{title, rows, compact, summary}` object onto the
// tile's channel. Rendering mirrors the desktop editor: detail rows with a
// state dot and a thin percent bar, a vertical provider stack with counts
// when the tile cannot fit the detail, and a bar+percent mini view on 1x1.

/** Parses the status payload; null when the value is not one (plain
 *  arrays keep rendering through [listItems] as a plain option list). */
fun statusData(tile: Tile, live: kotlinx.serialization.json.JsonElement?): StatusData? {
    val obj = live as? JsonObject ?: return null
    val rows = (obj["rows"] as? JsonArray)?.mapNotNull { el ->
        val r = el as? JsonObject ?: return@mapNotNull null
        fun str(key: String) = r[key]?.jsonPrimitive?.contentOrNull.orEmpty()
        val state = str("state")
        StatusRow(
            label = str("label"),
            value = str("value"),
            state = state.ifEmpty { "off" },
            percent = r["percent"]?.jsonPrimitive?.doubleOrNull,
            provider = str("provider"),
            isHeader = state == "header",
        )
    }.orEmpty()
    if (rows.isEmpty()) return null
    val compact = (obj["compact"] as? JsonArray)?.mapNotNull { el ->
        val c = el as? JsonObject ?: return@mapNotNull null
        val provider = c["provider"]?.jsonPrimitive?.contentOrNull ?: return@mapNotNull null
        StatusCompact(
            provider = provider,
            count = c["count"]?.jsonPrimitive?.intOrNull ?: 0,
            state = c["state"]?.jsonPrimitive?.contentOrNull ?: "working",
        )
    }.orEmpty()
    val summary = obj["summary"]?.jsonPrimitive?.contentOrNull.orEmpty()
    val rowStyle = obj["row_style"]?.jsonPrimitive?.contentOrNull ?: "name"
    return StatusData(rows, compact, summary, rowStyle)
}

/** "GLM 5h" -> "zcode": plan lane labels name the provider family. */
fun laneProvider(label: String): String {
    val first = label.split(" ").firstOrNull()?.lowercase().orEmpty()
    return if (first == "glm") "zcode" else first
}

fun statusColor(state: String): Color = when (state) {
    "working", "ok" -> Color(0xFF2ECC71)
    "attention", "warn" -> Color(0xFFF39C12)
    "high", "error" -> Color(0xFFE74C3C)
    else -> Color(0xFF95A5A6)
}

/** Brand marks (24x24 paths) for the agent providers; the same official
 *  assets the desktop editor draws. */
private val providerPaths = mapOf(
    "zcode" to "M12.606 1.806l-1.677 2.388c-0.258 0.374-0.697 0.606-1.161 0.606h-9.162V1.794C0.594 1.806 12.606 1.806 12.606 1.806zM24 1.806L9.6 22.206 0 22.206 14.4 1.806zM11.394 22.206l1.69-2.4c0.258-0.374 0.697-0.606 1.161-0.606h9.149v3.006H11.394z",
    "claude" to "m4.7144 15.9555 4.7174-2.6471.079-.2307-.079-.1275h-.2307l-.7893-.0486-2.6956-.0729-2.3375-.0971-2.2646-.1214-.5707-.1215-.5343-.7042.0546-.3522.4797-.3218.686.0608 1.5179.1032 2.2767.1578 1.6514.0972 2.4468.255h.3886l.0546-.1579-.1336-.0971-.1032-.0972L6.973 9.8356l-2.55-1.6879-1.3356-.9714-.7225-.4918-.3643-.4614-.1578-1.0078.6557-.7225.8803.0607.2246.0607.8925.686 1.9064 1.4754 2.4893 1.8336.3643.3035.1457-.1032.0182-.0728-.164-.2733-1.3539-2.4467-1.445-2.4893-.6435-1.032-.17-.6194c-.0607-.255-.1032-.4674-.1032-.7285L6.287.1335 6.6997 0l.9957.1336.419.3642.6192 1.4147 1.0018 2.2282 1.5543 3.0296.4553.8985.2429.8318.091.255h.1579v-.1457l.1275-1.706.2368-2.0947.2307-2.6957.0789-.7589.3764-.9107.7468-.4918.5828.2793.4797.686-.0668.4433-.2853 1.8517-.5586 2.9021-.3643 1.9429h.2125l.2429-.2429.9835-1.3053 1.6514-2.0643.7286-.8196.85-.9046.5464-.4311h1.0321l.759 1.1293-.34 1.1657-1.0625 1.3478-.8804 1.1414-1.2628 1.7-.7893 1.36.0729.1093.1882-.0183 2.8535-.607 1.5421-.2794 1.8396-.3157.8318.3886.091.3946-.3278.8075-1.967.4857-2.3072.4614-3.4364.8136-.0425.0304.0486.0607 1.5482.1457.6618.0364h1.621l3.0175.2247.7892.522.4736.6376-.079.4857-1.2142.6193-1.6393-.3886-3.825-.9107-1.3113-.3279h-.1822v.1093l1.0929 1.0686 2.0035 1.8092 2.5075 2.3314.1275.5768-.3218.4554-.34-.0486-2.2039-1.6575-.85-.7468-1.9246-1.621h-.1275v.17l.4432.6496 2.3436 3.5214.1214 1.0807-.17.3521-.6071.2125-.6679-.1214-1.3721-1.9246L14.38 17.959l-1.1414-1.9428-.1397.079-.674 7.2552-.3156.3703-.7286.2793-.6071-.4614-.3218-.7468.3218-1.4753.3886-1.9246.3157-1.53.2853-1.9004.17-.6314-.0121-.0425-.1397.0182-1.4328 1.9672-2.1796 2.9446-1.7243 1.8456-.4128.164-.7164-.3704.0667-.6618.4008-.5889 2.386-3.0357 1.4389-1.882.929-1.0868-.0062-.1579h-.0546l-6.3385 4.1164-1.1293.1457-.4857-.4554.0608-.7467.2307-.2429 1.9064-1.3114Z",
    "codex" to "M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9894 5.9894 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.0729zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464zM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.872zm16.5963 3.8558L13.1038 8.364 15.1192 7.2a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.407-.667zm2.0107-3.0231l-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66zM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927.6813zm1.0976-2.3654l2.602-1.4998 2.6069 1.4998v2.9994l-2.5974 1.4997-2.6067-1.4997Z",
    "opencode" to "M22 24H2V0h20zM17 4.8H7v14.4h10z",
    "antigravity" to "M0.0 21.11 1.62 19.49 3.25 16.78 4.69 12.81 6.68 5.41 8.12 2.35 9.38 0.9 10.83 0.18 12.63 0.0 14.26 0.54 15.34 1.44 16.96 4.15 20.21 15.16 21.65 18.23 23.1 19.85 23.1 20.21 24.0 20.93 24.0 22.02 22.38 22.2 21.11 21.11 20.75 21.11 18.77 18.95 15.7 13.71 14.8 12.81 13.35 12.09 10.83 12.09 9.38 12.81 7.76 14.62 5.23 19.13 2.71 21.65 1.8 22.2 0.54 22.38 0.0 22.02Z",
)

@Composable
private fun ProviderGlyph(provider: String, size: Dp, alpha: Float = 0.85f, tint: Color = Color.White) {
    val key = provider.lowercase()
    val d = providerPaths[key] ?: return
    // androidx.core's parser is the version-stable way to turn SVG path
    // data into something compose draws; the paint is hoisted because the
    // draw lambda runs on every frame. Tinted white like the editor's
    // inline SVGs - a fresh Paint defaults to black, which vanished on
    // the dark tiles.
    val nativePath = remember(d) { androidx.core.graphics.PathParser.createPathFromPathData(d) }
    val paint = remember(alpha, tint) {
        android.graphics.Paint().apply {
            style = android.graphics.Paint.Style.FILL
            isAntiAlias = true
            color = tint.copy(alpha = alpha).toArgb()
        }
    }
    Canvas(Modifier.size(size)) {
        scale(size.toPx() / 24f) {
            drawIntoCanvas { canvas ->
                canvas.nativeCanvas.drawPath(nativePath, paint)
            }
        }
    }
}

@Composable
private fun StatusDot(state: String) {
    Box(
        Modifier
            .size(8.dp)
            .clip(RoundedCornerShape(50))
            .background(statusColor(state)),
    )
}

@Composable
fun StatusTile(
    tile: Tile,
    data: StatusData,
    titleColor: Color,
    modifier: Modifier = Modifier,
) {
    val compact = data.compact.isNotEmpty() &&
        (tile.h <= 1 || data.rows.size > tile.h * 4)
    val mini = !compact && tile.w <= 1 && tile.h <= 1 && data.rows.any { it.percent != null }
    when {
        compact -> StatusCompactView(data, titleColor, modifier)
        mini -> StatusMiniView(data, modifier)
        else -> StatusDetailView(data, titleColor, modifier)
    }
}

/** Detail: one dot-label-value row per entry, a thin percent bar under
 *  rows that carry one, and the producer's summary at the bottom. */
@Composable
private fun StatusDetailView(
    data: StatusData,
    titleColor: Color,
    modifier: Modifier = Modifier,
) {
    Column(
        modifier
            .fillMaxSize()
            .padding(horizontal = 9.dp, vertical = 6.dp),
        verticalArrangement = Arrangement.spacedBy(5.dp, Alignment.CenterVertically),
    ) {
        data.rows.forEach { row ->
            Column {
                Row(
                    Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    val glyphProvider = row.provider.ifEmpty { laneProvider(row.label) }
                    // dot pinned to the left edge; the identifier is the
                    // text label OR the brand mark, never both (the
                    // producer's row_style switch, defaulting to name)
                    val showMark = !row.isHeader &&
                        data.rowStyle == "logo" &&
                        providerPaths.containsKey(glyphProvider.lowercase())
                    if (!row.isHeader) {
                        StatusDot(row.state)
                        Spacer(Modifier.size(7.dp))
                        if (showMark) {
                            ProviderGlyph(glyphProvider, 17.dp, alpha = 0.72f)
                            Spacer(Modifier.size(7.dp))
                        }
                    }
                    if (!showMark) {
                        Text(
                            text = row.label,
                            fontSize = if (row.isHeader) 9.5.sp else 11.sp,
                            fontWeight = if (row.isHeader) FontWeight.Bold else FontWeight.SemiBold,
                            letterSpacing = if (row.isHeader) 0.6.sp else 0.sp,
                            color = titleColor.copy(alpha = if (row.isHeader) 0.55f else 1f),
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f, fill = false),
                        )
                    }
                    if (row.value.isNotEmpty()) {
                        Spacer(Modifier.size(6.dp))
                        Text(
                            text = row.value,
                            fontSize = 10.5.sp,
                            color = when (row.state) {
                                "working", "ok" -> statusColor(row.state)
                                "attention", "warn" -> statusColor(row.state)
                                "done" -> Color.White.copy(alpha = 0.55f)
                                else -> Color.White.copy(alpha = 0.85f)
                            },
                            maxLines = 1,
                        )
                    }
                }
                row.percent?.let { percent ->
                    Box(
                        Modifier
                            .fillMaxWidth()
                            .padding(top = 3.dp)
                            .height(2.dp)
                            .clip(RoundedCornerShape(1.dp))
                            .background(Color.White.copy(alpha = 0.15f)),
                    ) {
                        Box(
                            Modifier
                                .fillMaxWidth((percent.coerceIn(0.0, 100.0) / 100.0).toFloat())
                                .height(2.dp)
                                .background(statusColor(row.state)),
                        )
                    }
                }
            }
        }
        if (data.summary.isNotEmpty()) {
            Column {
                Box(
                    Modifier
                        .fillMaxWidth()
                        .height(1.dp)
                        .background(Color.White.copy(alpha = 0.14f)),
                )
                Text(
                    text = data.summary,
                    fontSize = 10.5.sp,
                    color = Color.White.copy(alpha = 0.8f),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.padding(top = 4.dp),
                )
            }
        }
    }
}

/** Compact: vertical provider stack, each logo with its dot and the
 *  active+waiting count right below it. */
@Composable
private fun StatusCompactView(
    data: StatusData,
    titleColor: Color,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier.fillMaxSize(),
        horizontalArrangement = Arrangement.spacedBy(20.dp, Alignment.CenterHorizontally),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        data.compact.forEach { c ->
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                ProviderGlyph(c.provider, 34.dp)
                Row(
                    Modifier.padding(top = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    StatusDot(c.state)
                    Spacer(Modifier.size(5.dp))
                    Text(
                        text = c.count.toString(),
                        fontSize = 12.sp,
                        fontWeight = FontWeight.Bold,
                        color = Color.White.copy(alpha = 0.9f),
                    )
                }
            }
        }
    }
}

/** Mini (1x1 plan tiles): glyph + accent bar + percentage per window. */
@Composable
private fun StatusMiniView(data: StatusData, modifier: Modifier = Modifier) {
    val accent = Color(0xFF1ABC9C)
    Column(
        modifier
            .fillMaxSize()
            .padding(8.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp, Alignment.CenterVertically),
    ) {
        data.rows.forEach { row ->
            val percent = row.percent ?: return@forEach
            Row(
                Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                ProviderGlyph(row.provider.ifEmpty { laneProvider(row.label) }, 12.dp, alpha = 0.8f)
                Box(
                    Modifier
                        .weight(1f)
                        .height(8.dp)
                        .clip(RoundedCornerShape(2.dp))
                        .background(accent.copy(alpha = 0.18f)),
                ) {
                    Box(
                        Modifier
                            .fillMaxWidth((percent.coerceIn(0.0, 100.0) / 100.0).toFloat())
                            .height(8.dp)
                            .background(accent),
                    )
                }
                Text(
                    text = "${percent.roundToInt()}%",
                    fontSize = 11.5.sp,
                    fontWeight = FontWeight.Bold,
                    color = accent,
                )
            }
        }
    }
}
