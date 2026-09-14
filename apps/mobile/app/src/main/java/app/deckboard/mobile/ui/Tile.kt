//! Tile rendering compatible with the stock client: hex colors from the
//! payload, FontAwesome unicode icons, dual-state toggles, drag sliders
//! and custom-value/graph displays.

package app.deckboard.mobile.ui

import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
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
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.unit.times
import app.deckboard.mobile.R
import app.deckboard.mobile.proto.Shortcut

val FaBrands = FontFamily(Font(R.font.fa_brands_400))
val FaSolid = FontFamily(Font(R.font.fa_solid_900))

fun faFamily(prefix: String): FontFamily = if (prefix == "fab") FaBrands else FaSolid

fun hex(color: String, fallback: Color): Color =
    runCatching { Color(android.graphics.Color.parseColor(color.trim())) }
        .getOrDefault(fallback)

/** shape: 0 = square, 1 = rounded, 2 = circle. Square tiles still get a
 *  small corner radius so the grid reads softly on a tablet. */
private fun shapeOf(shape: Int, radius: Float): Shape = when (shape) {
    1 -> RoundedCornerShape(radius)
    2 -> CircleShape
    else -> RoundedCornerShape(radius / 2f)
}

@Composable
fun Tile(
    shortcut: Shortcut,
    tileSize: androidx.compose.ui.unit.Dp,
    customValue: String?,
    suffix: String?,
    history: List<Float>,
    listItems: List<String>,
    position: Int,
    onPressStart: () -> Unit,
    onPressEnd: () -> Unit,
    onToggle: () -> Unit,
    onSlider: (Float) -> Unit,
    modifier: Modifier = Modifier,
) {
    val active = position == 1
    val color = hex(if (active) shortcut.color2.ifEmpty { shortcut.color } else shortcut.color, DeckColors.tileFallback)
    val borderColor = hex(if (active) shortcut.borderColor2.orEmpty() else shortcut.borderColor.orEmpty(), Color.Transparent)
    val shape = shapeOf(if (active) shortcut.shape2 else shortcut.shape, tileSize.value * 0.18f)
    val unicode = if (active) shortcut.unicode2.ifEmpty { shortcut.unicode } else shortcut.unicode
    val iconColor = hex(
        (if (active) shortcut.iconColor2.orEmpty() else shortcut.iconColor.orEmpty()).ifEmpty { "#ffffff" },
        Color.White,
    )
    val titleColor = hex(if (active) shortcut.titleColor2 else shortcut.titleColor, Color.White)

    // physical key feedback: tiles sit raised with a hard shadow cast to
    // the bottom-right; pressing sinks the face (scale + shift toward the
    // shadow) and softens the shadow, release pops back with a small
    // bounce so it reads like a real key springing up
    var pressed by remember(shortcut.id) { mutableStateOf(false) }
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
            .aspectRatio(shortcut.w.toFloat() / shortcut.h.toFloat())
            .padding(3.dp),
    ) {
        // hard offset shadow behind the face, not clipped so it bleeds
        // into the grid gap like a real drop shadow
        Box(
            Modifier
                .matchParentSize()
                .offset(x = 4.dp, y = 4.dp)
                .clip(shape)
                .background(Color.Black.copy(alpha = shadowAlpha)),
        )
        Box(
            Modifier
                .matchParentSize()
                .offset(x = sink, y = sink)
                .clip(shape)
                .background(color)
                .then(
                    if (borderColor != Color.Transparent) {
                        Modifier.border(2.dp, borderColor, shape)
                    } else {
                        Modifier
                    },
                ),
            contentAlignment = Alignment.Center,
        ) {
            if (scrim > 0f) {
                Box(Modifier.matchParentSize().background(Color.White.copy(alpha = scrim)))
            }
            when (templateFor(shortcut)) {
                "slider" -> SliderTile(shortcut, color, iconColor, onSlider)
                "knob" -> KnobTile(shortcut, color, iconColor, titleColor, onSlider)
                "graph" -> GraphTile(shortcut, history, suffix, titleColor)
                "list" -> ListTile(shortcut, listItems, titleColor, onPress = onPressEnd)
                "custom-value" -> CustomValueTile(shortcut, customValue, suffix, titleColor)
                else -> ButtonTile(
                    shortcut = shortcut,
                    unicode = unicode,
                    iconColor = iconColor,
                    titleColor = titleColor,
                    onPressStart = {
                        pressed = true
                        onPressStart()
                    },
                    onPressEnd = {
                        pressed = false
                        onPressEnd()
                    },
                    onToggle = onToggle,
                )
            }
        }
    }
}

@Composable
private fun ButtonTile(
    shortcut: Shortcut,
    unicode: String,
    iconColor: Color,
    titleColor: Color,
    onPressStart: () -> Unit,
    onPressEnd: () -> Unit,
    onToggle: () -> Unit,
) {
    val title = shortcut.title.orEmpty()

    Box(
        Modifier
            .fillMaxSize()
            .pointerInput(shortcut.id) {
                detectTapGestures(
                    onPress = {
                        onPressStart()
                        try {
                            awaitRelease()
                        } finally {
                            onPressEnd()
                            if (shortcut.mode == "toggle") onToggle()
                        }
                    },
                )
            },
        contentAlignment = Alignment.Center,
    ) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            if (unicode.isNotEmpty()) {
                Text(
                    text = faChar(unicode),
                    fontFamily = faFamily(shortcut.prefix),
                    fontSize = 26.sp,
                    color = iconColor,
                )
            }
            if (title.isNotEmpty()) {
                Text(
                    text = title,
                    fontSize = 12.sp,
                    color = titleColor,
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
    shortcut: Shortcut,
    baseColor: Color,
    iconColor: Color,
    onSlider: (Float) -> Unit,
) {
    var value by remember(shortcut.id) { mutableFloatStateOf(0.5f) }
    val fill = if (shortcut.color2.isNotEmpty()) {
        hex(shortcut.color2, baseColor.copy(alpha = 0.6f))
    } else {
        baseColor.copy(alpha = 0.55f)
    }

    Box(
        Modifier
            .fillMaxSize()
            .pointerInput(shortcut.id) {
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
        if (shortcut.unicode.isNotEmpty()) {
            Text(
                text = faChar(shortcut.unicode),
                fontFamily = faFamily(shortcut.prefix),
                fontSize = 22.sp,
                color = iconColor,
            )
        }
    }
}

@Composable
private fun CustomValueTile(
    shortcut: Shortcut,
    customValue: String?,
    suffix: String?,
    titleColor: Color,
) {
    val display = listOfNotNull(customValue, suffix).joinToString("") { it }
    Column(
        Modifier
            .fillMaxSize()
            .padding(4.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = androidx.compose.foundation.layout.Arrangement.Center,
    ) {
        if (shortcut.unicode.isNotEmpty()) {
            Text(
                text = faChar(shortcut.unicode),
                fontFamily = faFamily(shortcut.prefix),
                fontSize = 20.sp,
                color = titleColor,
            )
        }
        Text(
            text = display,
            fontSize = 18.sp,
            color = titleColor,
            fontWeight = androidx.compose.ui.text.font.FontWeight.Bold,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            textAlign = TextAlign.Center,
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
    }
}

// the payload already carries the actual glyph character
fun faChar(unicode: String): String = unicode
