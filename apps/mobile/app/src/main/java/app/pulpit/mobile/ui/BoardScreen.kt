//! Board screen over protocol v2: tiles at (x, y) sized (w, h) on a
//! board.width x board.height grid. Live values and series come from the
//! server's channels keyed by the tile's state channel; board switches
//! arrive as `board.open` or via the floating chip.

package app.pulpit.mobile.ui

import android.os.Build

import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.State
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.SnapshotStateMap
import androidx.compose.runtime.structuralEqualityPolicy
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.blur
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.pulpit.mobile.net.ConnState
import app.pulpit.mobile.net.displayText
import app.pulpit.mobile.net.isActiveValue
import app.pulpit.mobile.net.numericValue
import app.pulpit.mobile.proto.Board
import app.pulpit.mobile.proto.ChannelInfo
import app.pulpit.mobile.proto.V2
import app.pulpit.mobile.state.PulpitViewModel
import kotlinx.serialization.json.JsonElement

@Composable
fun BoardScreen(vm: PulpitViewModel) {
    val board by vm.currentBoard.collectAsState()
    val conn by vm.connState.collectAsState()
    val boards by vm.boards.collectAsState()
    val attempt by vm.reconnectAttempt.collectAsState()

    val status = statusFor(conn, boards.isNotEmpty(), attempt)

    // the deck content sits in its own layer so a link overlay can blur it
    // where the platform supports it (RenderEffect needs API 31; older
    // devices get a deeper dim instead)
    Box(
        Modifier
            .fillMaxSize()
            .background(DeckColors.background)
            .then(if (status != null) Modifier.blur(16.dp) else Modifier),
    ) {
        BoardSwitcher(board, boards, vm, Modifier.fillMaxSize())
        Box(Modifier.align(Alignment.BottomCenter).padding(bottom = 6.dp)) {
            BoardChip(vm)
        }
    }

    if (status != null) {
        Box(Modifier.fillMaxSize()) {
            // full-screen dim; it also eats taps so a half-live deck cannot
            // accept gestures for frames that will never reach the server
            Box(
                Modifier
                    .matchParentSize()
                    .background(Color.Black.copy(alpha = if (Build.VERSION.SDK_INT >= 31) 0.45f else 0.72f))
                    .pointerInput(Unit) { detectTapGestures { } },
            )
            Column(
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(14.dp),
                modifier = Modifier.align(Alignment.Center).fillMaxWidth(),
            ) {
                RingSpinner()
                Text(
                    text = status,
                    color = Color.White,
                    style = MaterialTheme.typography.titleMedium,
                )
            }
        }
    }
}

/** Slide duration and the dim/parallax the settled board gets while the
 *  incoming one travels over it. */
private const val SWITCH_MS = 300f
private const val BASE_PARALLAX = 0.25f
private const val BASE_DIM = 0.35f

/** Animated board switch: the incoming board slides in from the side of
 *  its position in the boards list while the settled board drifts aside
 *  and dims. The progress loop reads raw frame time through
 *  `withFrameNanos`, because deck tablets routinely run with the system
 *  animator scales at 0 - that collapses every standard Compose animation
 *  into an instant jump (the frozen spinner on the SM-T561). Layer
 *  properties change per frame, so the boards themselves never
 *  recompose during the transition. */
@Composable
private fun BoardSwitcher(
    board: Board?,
    boards: List<Board>,
    vm: PulpitViewModel,
    modifier: Modifier = Modifier,
) {
    var settled by remember { mutableStateOf<Board?>(null) }
    var incoming by remember { mutableStateOf<Board?>(null) }
    var progress by remember { androidx.compose.runtime.mutableFloatStateOf(0f) }
    var fromRight by remember { mutableStateOf(true) }
    var widthPx by remember { androidx.compose.runtime.mutableIntStateOf(0) }

    LaunchedEffect(board?.id) {
        val target = board
        if (target == null) {
            settled = null
            incoming = null
            progress = 0f
            return@LaunchedEffect
        }
        val current = settled
        if (current == null || current.id == target.id) {
            // first board after (re)connect or a same-board refresh - no
            // transition. incoming is still cleared: this run may be
            // replacing a cancelled mid-flight switch (rapid board.open),
            // and a stale overlay would freeze on screen
            settled = target
            incoming = null
            progress = 0f
            return@LaunchedEffect
        }
        val order = { b: Board -> boards.indexOfFirst { it.id == b.id } }
        fromRight = order(target) >= order(current)
        incoming = target
        progress = 0f
        val start = withFrameNanos { it }
        while (true) {
            val now = withFrameNanos { it }
            val t = (now - start) / 1_000_000f / SWITCH_MS
                progress = FastOutSlowInEasing.transform(t.coerceIn(0f, 1f))
            if (t >= 1f) break
        }
        // Let the completed-slide frame present before swapping the
        // boards: the swap recomposes the whole settled board (36 tiles
        // on All In One) and stalls composition for hundreds of
        // milliseconds on the tablet. Deferred by one frame, that stall
        // lands on a static final image instead of freezing mid-slide.
        withFrameNanos { }
        settled = target
        incoming = null
        progress = 0f
    }

    // The live `board` object wins whenever its id matches the rendered
    // slot, so boards.delta edits keep flowing into the board on screen
    // (and into the one sliding in) instead of showing a stale snapshot.
    fun rendered(b: Board): Board = if (board?.id == b.id) board else b

    Box(modifier.onSizeChanged { widthPx = it.width }) {
        settled?.let { base ->
            val dim = if (incoming == null) 0f else progress
            Box(
                Modifier
                    .fillMaxSize()
                    .graphicsLayer {
                        val drift = BASE_PARALLAX * widthPx * dim
                        translationX = if (fromRight) -drift else drift
                    },
            ) {
                BoardGrid(vm, rendered(base), Modifier.fillMaxSize())
            }
            if (dim > 0f) {
                Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = BASE_DIM * dim)))
            }
        }
        incoming?.let { next ->
            BoardGrid(
                vm,
                rendered(next),
                Modifier
                    .fillMaxSize()
                    .graphicsLayer {
                        val side = if (fromRight) 1f else -1f
                        translationX = side * (1f - progress) * widthPx
                    },
            )
        }
    }
}

/** Center status for the link overlay, or null when the link is healthy. */
private fun statusFor(conn: ConnState, hasBoards: Boolean, attempt: Int): String? = when {
    conn is ConnState.Connected && !hasBoards -> "Syncing boards..."
    conn is ConnState.Connected -> null
    conn is ConnState.Connecting ->
        if (attempt > 0) "Reconnecting (attempt $attempt)..." else "Connecting..."
    else ->
        if (attempt > 0) "Disconnected - retrying (attempt $attempt)..."
        else "Disconnected - retrying..."
}

/** Small translucent board switcher in the corner - replaces the top bar. */
@Composable
private fun BoardChip(vm: PulpitViewModel) {
    val boards by vm.boards.collectAsState()
    val board by vm.currentBoard.collectAsState()
    var menuOpen by remember { mutableStateOf(false) }

    Box(Modifier.padding(10.dp)) {
        Text(
            text = board?.name ?: "Pulpit",
            fontSize = 12.sp,
            color = Color.White.copy(alpha = 0.75f),
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier
                .clip(RoundedCornerShape(14.dp))
                .background(Color.Black.copy(alpha = 0.35f))
                .clickable { menuOpen = true }
                .padding(horizontal = 12.dp, vertical = 6.dp),
        )
        DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }) {
            boards.forEach { b ->
                DropdownMenuItem(
                    text = { Text(b.name.ifEmpty { "board ${b.id}" }) },
                    onClick = {
                        menuOpen = false
                        vm.selectBoard(b)
                    },
                )
            }
        }
    }
}

@Composable
private fun BoardGrid(vm: PulpitViewModel, board: Board, modifier: Modifier) {
    // State holders, not snapshots: reading .value here would recompose
    // the whole grid on every state.patch (10 Hz while anything moves).
    // Each TileCell derives just its own channel from them.
    val values = vm.values.collectAsState()
    val series = vm.series.collectAsState()
    val channelMeta = vm.channelMeta.collectAsState()
    val bitmaps = vm.bitmaps.collectAsState()
    // toggles without a state channel keep client-side position state
    val positions = remember(board.id) { mutableStateMapOf<Long, Boolean>() }

    // a board image wins over the color (like the original client); the
    // color shows through while the asset loads
    val bgAsset = board.background?.hash
    LaunchedEffect(bgAsset) { bgAsset?.let { vm.ensureAsset(it) } }
    val bgBitmap by remember(bitmaps, bgAsset) {
        derivedStateOf { bgAsset?.let { bitmaps.value[it] } }
    }

    BoxWithConstraints(
        modifier.background(hex(board.background?.color, DeckColors.background)),
    ) {
        bgBitmap?.let {
            Image(
                bitmap = it,
                contentDescription = null,
                contentScale = ContentScale.Crop,
                modifier = Modifier.matchParentSize(),
            )
        }
        val tile = maxWidth / board.width.coerceAtLeast(1)
        val tileHeight = maxHeight / board.height.coerceAtLeast(1)

        board.tiles.forEach { t ->
            // identity by tile id, not list position: a delta that adds or
            // removes a tile must not hand its neighbors' animation and
            // gesture state to the wrong tile
            key(t.id) {
                Box(
                    Modifier
                        .offset(x = tile * t.x, y = tileHeight * t.y)
                        .width(tile * t.w)
                        .height(tileHeight * t.h),
                ) {
                    TileCell(vm, board.id, t, tile, values, series, channelMeta, bitmaps, positions)
                }
            }
        }
    }
}

/** One grid tile. Reads only its own channel, series window, and image,
 *  so a patch for another channel leaves it alone - the lambdas below
 *  re-run (a map lookup), the tile does not recompose. */
@Composable
private fun TileCell(
    vm: PulpitViewModel,
    boardId: Long,
    t: app.pulpit.mobile.proto.Tile,
    tileSize: Dp,
    values: State<Map<String, JsonElement>>,
    series: State<Map<String, List<Double>>>,
    channelMeta: State<Map<String, ChannelInfo>>,
    bitmaps: State<Map<String, ImageBitmap>>,
    positions: SnapshotStateMap<Long, Boolean>,
) {
    val watchChannel = t.state?.channel
    val live by remember(values, watchChannel) {
        derivedStateOf(structuralEqualityPolicy()) { watchChannel?.let { values.value[it] } }
    }
    val points by remember(series, watchChannel) {
        derivedStateOf(structuralEqualityPolicy()) {
            watchChannel?.let { series.value[it] } ?: emptyList()
        }
    }
    val meta by remember(channelMeta, watchChannel) {
        derivedStateOf(structuralEqualityPolicy()) { watchChannel?.let { channelMeta.value[it] } }
    }
    val image by remember(bitmaps, t.assetHash) {
        derivedStateOf { t.assetHash?.let { bitmaps.value[it] } }
    }
    val image2 by remember(bitmaps, t.assetHash2) {
        derivedStateOf { t.assetHash2?.let { bitmaps.value[it] } }
    }
    t.assetHash?.let { hash -> LaunchedEffect(hash) { vm.ensureAsset(hash) } }
    t.assetHash2?.let { hash -> LaunchedEffect(hash) { vm.ensureAsset(hash) } }
    val active = when {
        watchChannel != null -> isActiveValue(live)
        else -> positions[t.id] ?: false
    }
    Tile(
        tile = t,
        tileSize = tileSize,
        active = active,
        liveText = displayText(live),
        liveValue = numericValue(live),
        series = SeriesWindow(points),
        channel = meta,
        items = TileItems(listItems(t, live)),
        status = statusData(t, live),
        image = image,
        image2 = image2,
        onPressStart = { vm.pressStart(boardId, t) },
        onPressEnd = { vm.pressEnd(boardId, t) },
        onSlider = { v -> vm.slider(boardId, t, v) },
        onGesture = { name -> vm.gesture(boardId, t, name) },
    )
}
