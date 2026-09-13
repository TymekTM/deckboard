//! Widget templates (M5 widget kit): graph sparkline, knob and list.
//!
//! A tile uses a template when its manifest asks for one - the manifest
//! travels in the shortcut `options` field (`{"widget": "knob"}` etc.) -
//! or implicitly for `mode: "graph"`, which the original app renders as a
//! value history series.

package app.deckboard.mobile.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
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
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.deckboard.mobile.proto.Shortcut
import kotlinx.serialization.json.jsonPrimitive
import kotlin.math.atan2
import kotlin.math.ceil
import kotlin.math.cos
import kotlin.math.max
import kotlin.math.min
import kotlin.math.sin

private fun parse(value: Float?): String =
    if (value == null) "" else if (value == ceil(value)) value.toInt().toString() else "%.1f".format(value)

/**
 * Widget manifest carried in the shortcut `options` field. Missing or
 * unparsable options mean "no manifest" and the mode decides the template.
 */
data class WidgetManifest(val widget: String) {
    companion object {
        fun of(shortcut: Shortcut): WidgetManifest? {
            val el = shortcut.options ?: return null
            val obj = el as? kotlinx.serialization.json.JsonObject ?: return null
            val name = obj["widget"]?.let {
                runCatching { it.jsonPrimitive.content }.getOrNull()
            } ?: return null
            return WidgetManifest(name)
        }
    }
}

/** Implicit template for a shortcut when no manifest overrides it. */
fun templateFor(shortcut: Shortcut): String {
    val manifest = WidgetManifest.of(shortcut)
    if (manifest != null) return manifest.widget
    return when (shortcut.mode) {
        "graph" -> "graph"
        else -> shortcut.mode
    }
}

/** Line chart of the last values, with current value and title. */
@Composable
fun GraphTile(
    shortcut: Shortcut,
    history: List<Float>,
    titleColor: Color,
    modifier: Modifier = Modifier,
) {
    val latest = history.lastOrNull()
    val lineColor = titleColor.copy(alpha = 0.9f)
    Column(
        modifier
            .fillMaxSize()
            .padding(6.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(
            text = parse(latest),
            fontSize = 20.sp,
            fontWeight = FontWeight.Bold,
            color = titleColor,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        if (!shortcut.title.isNullOrEmpty()) {
            Text(
                text = shortcut.title,
                fontSize = 10.sp,
                color = titleColor.copy(alpha = 0.75f),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Canvas(Modifier.fillMaxWidth().fillMaxHeight().padding(top = 2.dp)) {
            if (history.size < 2) return@Canvas
            val minV = history.min()
            val maxV = history.max()
            val span = (maxV - minV).takeIf { it > 0f } ?: 1f
            val stepX = size.width / (history.size - 1)
            val path = Path()
            history.forEachIndexed { i, v ->
                val x = i * stepX
                val y = size.height - ((v - minV) / span) * size.height
                if (i == 0) path.moveTo(x, y) else path.lineTo(x, y)
            }
            drawPath(path, lineColor, style = Stroke(width = 3f, cap = StrokeCap.Round))
        }
    }
}

/**
 * Circular control: drag around the center to set a 0..1 value, sent via
 * the same `exec_slider` channel as slider tiles.
 */
@Composable
fun KnobTile(
    shortcut: Shortcut,
    baseColor: Color,
    iconColor: Color,
    titleColor: Color,
    onSlider: (Float) -> Unit,
    modifier: Modifier = Modifier,
) {
    var value by remember(shortcut.id) { mutableFloatStateOf(0.5f) }
    val arcColor = if (shortcut.color2.isNotEmpty()) hex(shortcut.color2, titleColor) else titleColor

    Box(
        modifier
            .fillMaxSize()
            .pointerInput(shortcut.id) {
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
            val angle = Math.toRadians((135.0 + 270.0 * value))
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
            text = parse(value),
            fontSize = 16.sp,
            fontWeight = FontWeight.Bold,
            color = titleColor,
        )
    }
}

/**
 * Scrollable option list. Items come from the manifest (`options.items`)
 * or from the live value when the extension pushes an array. Tapping an
 * item selects it and sends the button press (the extension sees the tap;
 * per-item args arrive with protocol v2).
 */
@Composable
fun ListTile(
    shortcut: Shortcut,
    items: List<String>,
    titleColor: Color,
    onPress: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var selected by remember(shortcut.id) { androidx.compose.runtime.mutableStateOf(-1) }
    Column(
        modifier
            .fillMaxSize()
            .padding(4.dp)
            .clip(RoundedCornerShape(6.dp))
            .background(titleColor.copy(alpha = 0.08f)),
    ) {
        if (!shortcut.title.isNullOrEmpty()) {
            Text(
                text = shortcut.title,
                fontSize = 11.sp,
                color = titleColor,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(start = 6.dp, top = 4.dp),
            )
        }
        LazyColumn(Modifier.fillMaxSize()) {
            itemsIndexed(items) { i, item ->
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
fun listItems(shortcut: Shortcut, live: kotlinx.serialization.json.JsonElement?): List<String> {
    val fromManifest = (shortcut.options as? kotlinx.serialization.json.JsonObject)
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
