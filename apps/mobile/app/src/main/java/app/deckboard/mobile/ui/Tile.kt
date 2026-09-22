//! Tile rendering over protocol v2 (docs/protocol-v2.md §4/§5): styles
//! arrive resolved (hex colors, unicode glyphs, active-state pairs), the
//! kind picks the template, unknown kinds degrade to a plain button.

package app.deckboard.mobile.ui

import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.unit.times
import app.deckboard.mobile.R
import app.deckboard.mobile.proto.ChannelInfo
import app.deckboard.mobile.proto.Tile
import app.deckboard.mobile.proto.V2
import kotlinx.serialization.json.JsonElement

val FaBrands = FontFamily(Font(R.font.fa_brands_400))
val FaSolid = FontFamily(Font(R.font.fa_solid_900))

fun faFamily(prefix: String?): FontFamily = if (prefix == "fab") FaBrands else FaSolid

fun hex(color: String?, fallback: Color): Color =
    color?.let { runCatching { Color(android.graphics.Color.parseColor(it.trim())) }.getOrNull() }
        ?: fallback

/** shape: 0 = square, 1 = rounded, 2 = circle. Square tiles still get a
 *  small corner radius so the grid reads softly on a tablet. */
private fun shapeOf(shape: Int, radius: Float): Shape = when (shape) {
    1 -> RoundedCornerShape(radius)
    2 -> CircleShape
    else -> RoundedCornerShape(radius / 2f)
}

/** Template for a tile: the kind decides, with `params.widget` hints
 *  (clock) on top. Unknown kinds degrade to a button. */
fun templateFor(tile: Tile): String = when {
    // a clock can arrive as a toggle (the clock extension pushes the time
    // onto a channel); the widget hint outranks the wire kind
    tile.widgetHint() == "clock" -> "clock"
    tile.kind == V2.KIND_SLIDER -> "slider"
    tile.kind == V2.KIND_KNOB -> "knob"
    tile.kind == V2.KIND_GRAPH -> "graph"
    tile.kind == V2.KIND_LIST -> "list"
    tile.kind == V2.KIND_TOGGLE -> "toggle"
    else -> "button"
}

@Composable
fun Tile(
    tile: Tile,
    tileSize: androidx.compose.ui.unit.Dp,
    active: Boolean,
    liveText: String?,
    series: SeriesWindow,
    channel: ChannelInfo? = null,
    items: TileItems,
    image: ImageBitmap? = null,
    onPressStart: () -> Unit,
    onPressEnd: () -> Unit,
    onSlider: (Float) -> Unit,
    modifier: Modifier = Modifier,
) {
    val style = tile.style
    // discord voice toggles carry the state like the official controls:
    // pushed OFF (muted / deafened) lights the face red and swaps in the
    // slash glyph - FA has no slashed headphones, the deaf ear reads as
    // deafened - while ON keeps the tile color and the open glyph
    val discordKind = when (tile.state?.channel?.removePrefix("ext.")) {
        "toggle-microphone" -> "mic"
        "toggle-headphone" -> "headphone"
        else -> null
    }
    val muted = discordKind != null && liveText == "OFF"
    val baseColor = hex(style?.color, DeckColors.tileFallback)
    val activeColor = hex(style?.color2, baseColor)
    val color = when {
        muted -> Color(0xFFED4245)
        active -> activeColor
        else -> baseColor
    }
    // active state swaps in the paired glyph (mic -> mic-slash, ...)
    val icon = when (discordKind) {
        "mic" -> if (muted) "" else ""
        "headphone" -> if (muted) "" else ""
        else -> (if (active) style?.icon2 ?: style?.icon else style?.icon).orEmpty()
    }
    val iconFamily = faFamily(style?.iconFamily)
    val iconColor = Color.White
    val titleColor = Color.White
    val title = style?.title.orEmpty()
    val shape = shapeOf(style?.shape?.toIntOrNull() ?: 0, tileSize.value * 0.18f)

    val template = templateFor(tile)
    val raised = template != "graph" && template != "clock"

    // physical key feedback: tiles sit raised with a hard shadow cast to
    // the bottom-right; pressing sinks the face (scale + shift toward the
    // shadow) and softens the shadow, release pops back with a small
    // bounce so it reads like a real key springing up
    var pressed by remember(tile.id) { mutableStateOf(false) }
    val scale by animateFloatAsState(
        targetValue = if (pressed) 0.96f else 1f,
        animationSpec = if (pressed) {
            tween(durationMillis = 80)
        } else {
            spring(dampingRatio = Spring.DampingRatioLowBouncy, stiffness = Spring.StiffnessMedium)
        },
        label = "tilePressScale",
    )
    val sink by animateDpAsState(
        targetValue = if (pressed) 2.dp else 0.dp,
        animationSpec = if (pressed) {
            tween(durationMillis = 80)
        } else {
            spring(dampingRatio = Spring.DampingRatioLowBouncy, stiffness = Spring.StiffnessMedium)
        },
        label = "tilePressSink",
    )
    val scrim by animateFloatAsState(
        targetValue = if (pressed) 0.12f else 0f,
        animationSpec = tween(durationMillis = 80),
        label = "tilePressScrim",
    )
    val shadowAlpha by animateFloatAsState(
        targetValue = if (pressed) 0.28f else 0.45f,
        animationSpec = tween(durationMillis = 80),
        label = "tilePressShadow",
    )

    Box(
        modifier
            .graphicsLayer {
                scaleX = scale
                scaleY = scale
            }
            .aspectRatio(tile.w.toFloat() / tile.h.toFloat())
            .padding(3.dp),
    ) {
        // hard offset shadow behind the face, not clipped so it bleeds
        // into the grid gap like a real drop shadow
        if (raised) {
            Box(
                Modifier
                    .matchParentSize()
                    .offset(x = 4.dp, y = 4.dp)
                    .clip(shape)
                    .background(Color.Black.copy(alpha = shadowAlpha)),
            )
        }
        Box(
            Modifier
                .matchParentSize()
                .offset(x = sink, y = sink)
                .clip(shape)
                .background(color)
                .border(0.dp, Color.Transparent),
            contentAlignment = Alignment.Center,
        ) {
            if (scrim > 0f) {
                Box(Modifier.matchParentSize().background(Color.White.copy(alpha = scrim)))
            }
            when (template) {
                "slider" -> SliderTile(tile, color, icon, iconFamily, iconColor, onSlider)
                "knob" -> KnobTile(tile, titleColor, iconColor, titleColor, onSlider)
                "graph" -> GraphTile(tile, series, liveText, channel, titleColor)
                "clock" -> ClockTile(tile, icon, iconFamily, titleColor)
                "list" -> ListTile(tile, items, titleColor, onPress = onPressEnd)
                else -> ButtonTile(
                    tile = tile,
                    unicode = icon,
                    iconFamily = iconFamily,
                    iconColor = iconColor,
                    titleColor = titleColor,
                    image = image,
                    iconOnly = discordKind != null,
                    liveText = if (template == "toggle" && discordKind == null) liveText else null,
                    onPressStart = {
                        pressed = true
                        onPressStart()
                    },
                    onPressEnd = {
                        pressed = false
                        onPressEnd()
                    },
                )
            }
        }
    }
}

/** Button (and toggle) tiles: press feedback per the declared gestures. */
@Composable
private fun ButtonTile(
    tile: Tile,
    unicode: String,
    iconFamily: FontFamily,
    iconColor: Color,
    titleColor: Color,
    image: ImageBitmap?,
    iconOnly: Boolean,
    liveText: String?,
    onPressStart: () -> Unit,
    onPressEnd: () -> Unit,
) {
    // icon-only faces (discord voice toggles): the color and the glyph
    // carry the state, a label would only repeat it
    val title = if (iconOnly) "" else listOfNotNull(
        liveText,
        tile.style?.title,
    ).filter { it.isNotEmpty() }.joinToString("  ")

    Box(
        Modifier
            .fillMaxSize()
            .pointerInput(tile.id) {
                detectTapGestures(
                    onPress = {
                        onPressStart()
                        try {
                            awaitRelease()
                        } finally {
                            onPressEnd()
                        }
                    },
                )
            },
        contentAlignment = Alignment.Center,
    ) {
            // an attached image replaces the glyph as the face; the title
            // stays so the key still reads at a glance
            if (image != null) {
                Image(
                    bitmap = image,
                    contentDescription = tile.style?.title,
                    contentScale = ContentScale.Crop,
                    modifier = Modifier.matchParentSize(),
                )
            }
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            if (image == null && unicode.isNotEmpty()) {
                Text(
                    text = faChar(unicode),
                    fontFamily = iconFamily,
                    fontSize = 26.sp,
                    color = iconColor,
                )
            }
            if (title.isNotEmpty()) {
                Text(
                    text = title,
                    fontSize = if (liveText != null) 18.sp else 12.sp,
                    color = titleColor,
                    fontWeight = if (liveText != null) {
                        androidx.compose.ui.text.font.FontWeight.Bold
                    } else {
                        androidx.compose.ui.text.font.FontWeight.Normal
                    },
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                    textAlign = TextAlign.Center,
                    modifier = Modifier.padding(horizontal = 4.dp),
                )
            }
        }
    }
}

@Composable
private fun SliderTile(
    tile: Tile,
    baseColor: Color,
    icon: String,
    iconFamily: FontFamily,
    iconColor: Color,
    onSlider: (Float) -> Unit,
) {
    var value by remember(tile.id) { mutableFloatStateOf(0.5f) }
    val fill = tile.style?.color2?.let { hex(it, baseColor.copy(alpha = 0.6f)) }
        ?: baseColor.copy(alpha = 0.55f)

    Box(
        Modifier
            .fillMaxSize()
            .pointerInput(tile.id) {
                detectDragGestures(
                    onDragStart = { offset ->
                        value = (1f - offset.y / size.height).coerceIn(0f, 1f)
                        onSlider(value)
                    },
                    onDrag = { change, _ ->
                        change.consume()
                        value = (1f - change.position.y / size.height).coerceIn(0f, 1f)
                        onSlider(value)
                    },
                )
            },
        contentAlignment = Alignment.BottomCenter,
    ) {
        Box(
            Modifier
                .fillMaxWidth()
                .fillMaxHeight(value)
                .background(fill),
        )
        if (icon.isNotEmpty()) {
            Text(
                text = faChar(icon),
                fontFamily = iconFamily,
                fontSize = 22.sp,
                color = iconColor,
            )
        }
    }
}

// the payload already carries the actual glyph character
fun faChar(unicode: String): String = unicode
