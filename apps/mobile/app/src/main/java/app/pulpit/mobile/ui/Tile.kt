//! Tile rendering over protocol v2 (docs/protocol-v2.md §4/§5): styles
//! arrive resolved (hex colors, unicode glyphs, active-state pairs), the
//! kind picks the template, unknown kinds degrade to a plain button.

package app.pulpit.mobile.ui

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
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.foundation.layout.Box
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
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
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
import app.pulpit.mobile.R
import app.pulpit.mobile.proto.ChannelInfo
import app.pulpit.mobile.proto.Tile
import app.pulpit.mobile.proto.V2
import kotlinx.serialization.json.JsonElement

val FaBrands = FontFamily(Font(R.font.fa_brands_400))
val FaSolid = FontFamily(Font(R.font.fa_solid_900))

fun faFamily(prefix: String?): FontFamily = if (prefix == "fab") FaBrands else FaSolid

fun hex(color: String?, fallback: Color): Color =
    color?.let { runCatching { Color(android.graphics.Color.parseColor(it.trim())) }.getOrNull() }
        ?: fallback

/** Shape mapping mirrors the desktop's TileCell exactly
 *  (`shape === 1 ? "50%" : "8px"`): 1 = circle, anything else (0,
 *  unknown values) = the soft 8dp corner square tiles get. */
private fun shapeOf(shape: Int): Shape = when (shape) {
    1 -> CircleShape
    else -> RoundedCornerShape(8.dp)
}

/** Title pinning inside the tile, mirroring the desktop's pos-* classes:
 * 0 = bottom (default), 1 = center, 2 = top. */
private fun titleAlignment(pos: Int): Alignment = when (pos) {
    2 -> Alignment.TopCenter
    1 -> Alignment.Center
    else -> Alignment.BottomCenter
}

/** Template for a tile: the kind decides, with `params.widget` hints
 *  (clock) on top. Unknown kinds degrade to a button. */
fun templateFor(tile: Tile): String = when {
    // a clock can arrive as a toggle (the clock extension pushes the time
    // onto a channel); the widget hint outranks the wire kind
    tile.widgetHint() == "clock" -> "clock"
    tile.widgetHint() == "tool-clock" -> "tool-clock"
    tile.widgetHint() == "tool-timer" -> "tool-timer"
    tile.widgetHint() == "tool-stopwatch" -> "tool-stopwatch"
    tile.widgetHint() == "tool-counter" -> "tool-counter"
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
    /** The channel's numeric live value, when it carries one: sliders and
     *  knobs position themselves from it until the user drags. */
    liveValue: Double?,
    series: SeriesWindow,
    channel: ChannelInfo? = null,
    items: TileItems,
    status: StatusData? = null,
    image: ImageBitmap? = null,
    image2: ImageBitmap? = null,
    /** Album art of a status payload (the payload's own asset hash,
     *  distinct from the tile face images above). */
    statusImage: ImageBitmap? = null,
    /** Local receive time of the status payload, for its playback
     *  progress extrapolation. */
    statusReceivedAtMs: Long = 0L,
    /** The raw live value on the tile's state channel, when it carries
     *  one: tool tiles (timer/stopwatch/counter) read their compact
     *  server state object out of it. */
    liveElement: JsonElement? = null,
    onPressStart: () -> Unit,
    onPressEnd: () -> Unit,
    onSlider: (Float) -> Unit,
    onGesture: (String) -> Unit = {},
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
    // state 2 falls back to state 1 per field (docs/protocol-v2.md §4):
    // an absent active-state value keeps the resting one instead of
    // resetting to a built-in default
    fun pick(first: String?, second: String?): String? =
        if (active) second ?: first else first

    val iconColor = hex(pick(style?.iconColor, style?.iconColor2), Color.White)
    val titleColor = hex(pick(style?.titleColor, style?.titleColor2), Color.White)
    val borderColor = pick(style?.borderColor, style?.borderColor2)
    val title = style?.title.orEmpty()
    // active state swaps the shape pair too (state 2 falls back to
    // state 1 per field, §4); shapes travel as stringified ints
    val shapeValue = pick(style?.shape, style?.shape2)?.toIntOrNull() ?: 0
    val shape = shapeOf(shapeValue)
    // title pinning + box color with the same per-field state-2 fallback
    fun pickInt(first: Int?, second: Int?): Int? = if (active) second ?: first else first
    val titlePos = pickInt(style?.titlePosition, style?.titlePosition2) ?: 0
    val titleBox = pick(style?.titleBoxColor, style?.titleBoxColor2)?.let { hex(it, Color.Transparent) }
    // the active-state image (img2) replaces the resting face while the
    // tile reads its active value; absent falls back to the resting one
    val face = if (active) image2 ?: image else image

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
                .border(
                    if (borderColor != null) 2.dp else 0.dp,
                    hex(borderColor, Color.Transparent),
                ),
            contentAlignment = Alignment.Center,
        ) {
            if (scrim > 0f) {
                Box(Modifier.matchParentSize().background(Color.White.copy(alpha = scrim)))
            }
            when (template) {
                "tool-clock" -> ToolClockTile(tile, titleColor)
                "tool-timer" -> ToolTimerTile(tile, liveElement, titleColor, onPress = onPressEnd, onGesture = onGesture)
                "tool-stopwatch" -> ToolStopwatchTile(tile, liveElement, titleColor, onPress = onPressEnd, onGesture = onGesture)
                "tool-counter" -> ToolCounterTile(tile, liveElement, titleColor, onPress = onPressEnd, onGesture = onGesture)
                "slider" -> SliderTile(tile, color, icon, iconFamily, iconColor, liveValue, onSlider)
                "knob" -> KnobTile(tile, iconColor, titleColor, liveValue, onSlider)
                "graph" -> GraphTile(tile, series, liveText, channel, titleColor)
                "clock" -> ClockTile(tile, icon, iconFamily, titleColor)
                "list" ->
                    if (status != null) {
                        // ai-dev status push: rows/compact/summary renderer
                        StatusTile(
                            tile,
                            status,
                            titleColor,
                            art = statusImage,
                            receivedAtMs = statusReceivedAtMs,
                        )
                    } else {
                        ListTile(tile, items, titleColor, onPress = onPressEnd)
                    }
                else -> ButtonTile(
                    tile = tile,
                    unicode = icon,
                    iconFamily = iconFamily,
                    iconColor = iconColor,
                    titleColor = titleColor,
                    titlePos = titlePos,
                    titleBox = titleBox,
                    image = face,
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
                    onPressCancel = {
                        // a cancelled touch resets the face without firing
                        pressed = false
                    },
                    onGesture = onGesture,
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
    titlePos: Int,
    titleBox: Color?,
    image: ImageBitmap?,
    iconOnly: Boolean,
    liveText: String?,
    onPressStart: () -> Unit,
    onPressEnd: () -> Unit,
    onPressCancel: () -> Unit,
    onGesture: (String) -> Unit,
) {
    // the gesture block below lives as long as tile.id; a live tile edit
    // (board.delta) swaps the callbacks underneath it, so read the newest
    val pressStart by rememberUpdatedState(onPressStart)
    val pressEnd by rememberUpdatedState(onPressEnd)
    val onGestureLatest by rememberUpdatedState(onGesture)
    // the tile object swaps too (an edit can change press modes): the
    // cancel decision must use the current interactions, not the ones
    // from the composition that started the gesture
    val currentTile by rememberUpdatedState(tile)
    // M5 custom gestures: the tap detector gains long-press/double-tap
    // when the tile declares them, and a passive drag observer classifies
    // declared swipes. The tracker suppresses the release tap once a
    // gesture spoke, so one touch fires exactly one interaction.
    val tracker = remember(tile.id) { GestureTracker() }
    // MOB-03: a tile that declares double-tap without the press pair
    // takes its tap from detectTapGestures' onTap - the detector
    // suppresses the first tap when a second lands inside the window,
    // which the release path cannot do (tryAwaitRelease returns before
    // onDoubleTap can mark the tracker). Press-mode tiles keep the
    // release path: their release sends press-end, never a tap.
    val tapFromDetector = tile.interacts(V2.INT_DOUBLE_TAP) &&
        !tile.interacts(V2.INT_PRESS_END)
    val haptics = LocalHaptics.current
    val wantsSwipe =
        tile.interacts(V2.INT_SWIPE_LEFT) || tile.interacts(V2.INT_SWIPE_RIGHT)
    // icon-only faces (discord voice toggles): the color and the glyph
    // carry the state, a label would only repeat it
    val title = if (iconOnly) "" else listOfNotNull(
        liveText,
        tile.style?.title,
    ).filter { it.isNotEmpty() }.joinToString("  ")

    Box(
        Modifier
            .fillMaxSize()
            .then(
                if (wantsSwipe) {
                    Modifier.pointerInput(tile.id) {
                        // passive observer: measures the whole drag and
                        // classifies it on release; the tap detector still
                        // sees the touch, the tracker just marks it spent
                        var total = Offset.Zero
                        detectDragGestures(
                            onDragStart = {
                                tracker.reset()
                                total = Offset.Zero
                            },
                            onDrag = { change, _ ->
                                total += change.positionChange()
                            },
                            onDragEnd = {
                                tracker.moved = true
                                val swipe = classifySwipe(total.x, total.y, SWIPE_MIN_PX)
                                if (swipe.isRecognized) {
                                    haptics.heavy()
                                }
                                when (swipe) {
                                    Swipe.Left -> onGestureLatest(V2.INT_SWIPE_LEFT)
                                    Swipe.Right -> onGestureLatest(V2.INT_SWIPE_RIGHT)
                                    Swipe.None -> {}
                                }
                            },
                            onDragCancel = { tracker.moved = true },
                        )
                    }
                } else {
                    Modifier
                }
            )
            .pointerInput(tile.id, tile.interactions) {
                detectTapGestures(
                    onLongPress = if (tile.interacts(V2.INT_LONG_PRESS)) {
                        { _ ->
                            tracker.longPressed = true
                            haptics.heavy()
                            onGestureLatest(V2.INT_LONG_PRESS)
                        }
                    } else {
                        null
                    },
                    onDoubleTap = if (tile.interacts(V2.INT_DOUBLE_TAP)) {
                        { _ ->
                            tracker.doubleTapped = true
                            haptics.heavy()
                            onGestureLatest(V2.INT_DOUBLE_TAP)
                        }
                    } else {
                        null
                    },
                    // only registered for the MOB-03 tiles above; when
                    // onDoubleTap is registered Compose defers the first
                    // tap through the double-tap window for us
                    onTap = if (tapFromDetector) {
                        { pressEnd() }
                    } else {
                        null
                    },
                    onPress = {
                        tracker.reset()
                        haptics.click()
                        pressStart()
                        var released = false
                        try {
                            // tryAwaitRelease is false when the touch was
                            // cancelled (finger slid off, parent stole it)
                            released = tryAwaitRelease()
                        }                         finally {
                            // a long press or a swipe already sent this
                            // touch's interaction; the plain release must
                            // not fire a second one - but a press-mode
                            // tile still owes its press-end even then
                            // (MOB-02): only press-end stops the server's
                            // hold-repeat loop, so the release decision
                            // handles consumed gestures explicitly
                            when (
                                releaseDecision(
                                    consumed = tracker.consumed,
                                    released = released,
                                    declaresPressEnd = currentTile.interacts(V2.INT_PRESS_END),
                                    declaresPressPair = currentTile.interacts(V2.INT_PRESS_START) &&
                                        currentTile.interacts(V2.INT_PRESS_END),
                                    tapSentFromOnTap = currentTile.interacts(V2.INT_DOUBLE_TAP) &&
                                        !currentTile.interacts(V2.INT_PRESS_END),
                                )
                            ) {
                                ReleaseDecision.Fire -> pressEnd()
                                ReleaseDecision.Cancel -> onPressCancel()
                            }
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
            // the glyph stays centered like the desktop's tile-icon; the
            // title is pinned per title_position (bottom/center/top) with
            // the optional title_box_color as a full-width strip behind
            // it - the same pos-* layout TileCell.vue draws
            if (image == null && unicode.isNotEmpty()) {
                Text(
                    text = unicode,
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
                    modifier = Modifier
                        .align(titleAlignment(titlePos))
                        .then(
                            titleBox?.let { Modifier.fillMaxWidth().background(it) }
                                ?: Modifier
                        )
                        .padding(horizontal = 4.dp, vertical = if (titleBox != null) 1.dp else 0.dp),
                )
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
    liveValue: Double?,
    onSlider: (Float) -> Unit,
) {
    // one drag protocol with the knob template (MOB-13): see
    // SlideDragController for the live-echo and convergence policy
    val haptics = LocalHaptics.current
    val onStepTick: () -> Unit = remember(haptics) { { haptics.sliderTick() } }
    val slide = remember(tile.id) { SlideDragController() }
    // see ButtonTile above: the drag block outlives a live tile edit
    val live by rememberUpdatedState(liveValue)
    val sendSlide by rememberUpdatedState(onSlider)
    val value = slide.current(liveValue)
    val fill = tile.style?.color2?.let { hex(it, baseColor.copy(alpha = 0.6f)) }
        ?: baseColor.copy(alpha = 0.55f)

    Box(
        Modifier
            .fillMaxSize()
            .pointerInput(tile.id) {
                detectDragGestures(
                    onDragStart = { offset ->
                        slide.start((1f - offset.y / size.height).coerceIn(0f, 1f), sendSlide)
                    },
                    onDrag = { change, _ ->
                        change.consume()
                        slide.move((1f - change.position.y / size.height).coerceIn(0f, 1f), onStepTick, sendSlide)
                    },
                    onDragEnd = {
                        // converge: the last sampled value always reaches
                        // the server, throttling only smooths the path
                        slide.end(live, sendSlide)
                    },
                    onDragCancel = {
                        // a cancelled drag still commits its last sampled
                        // position (like the desktop), then follows live
                        slide.cancel(live, sendSlide)
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
                text = icon,
                fontFamily = iconFamily,
                fontSize = 22.sp,
                color = iconColor,
            )
        }
    }
}
