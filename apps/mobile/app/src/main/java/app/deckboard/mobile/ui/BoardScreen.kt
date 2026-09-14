//! Board screen over protocol v2: tiles at (x, y) sized (w, h) on a
//! board.width x board.height grid. Live values and series come from the
//! server's channels keyed by the tile's state channel; board switches
//! arrive as `board.open` or via the floating chip.

package app.deckboard.mobile.ui

import android.os.Build

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
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.blur
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.deckboard.mobile.net.ConnState
import app.deckboard.mobile.net.displayText
import app.deckboard.mobile.net.isActiveValue
import app.deckboard.mobile.proto.Board
import app.deckboard.mobile.proto.V2
import app.deckboard.mobile.state.DeckboardViewModel

@Composable
fun BoardScreen(vm: DeckboardViewModel) {
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
        board?.let { b ->
            BoardGrid(vm, b, Modifier.fillMaxSize())
        }
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

/** Static busy ring: deck tablets often run with animator scales off,
 *  which freezes an indeterminate spinner into an invisible dot. A fixed
 *  300-degree arc reads as "busy" on every device. */
@Composable
private fun RingSpinner(modifier: Modifier = Modifier) {
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
private fun BoardChip(vm: DeckboardViewModel) {
    val boards by vm.boards.collectAsState()
    val board by vm.currentBoard.collectAsState()
    var menuOpen by remember { mutableStateOf(false) }

    Box(Modifier.padding(10.dp)) {
        Text(
            text = board?.name ?: "Deckboard",
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
private fun BoardGrid(vm: DeckboardViewModel, board: Board, modifier: Modifier) {
    val liveValues by vm.values.collectAsState()
    val series by vm.series.collectAsState()
    val bitmaps by vm.bitmaps.collectAsState()
    // toggles without a state channel keep client-side position state
    val positions = remember(board.id) { mutableStateMapOf<Long, Boolean>() }

    // a board image wins over the color (like the original client); the
    // color shows through while the asset loads
    val bgAsset = board.background?.hash
    LaunchedEffect(bgAsset) { bgAsset?.let { vm.ensureAsset(it) } }
    val bgBitmap = bgAsset?.let { bitmaps[it] }

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
            val watchChannel = t.state?.channel
            val live = liveValues[watchChannel]
            t.assetHash?.let { hash -> LaunchedEffect(hash) { vm.ensureAsset(hash) } }
            val active = when {
                watchChannel != null -> isActiveValue(live)
                else -> positions[t.id] ?: false
            }
            Box(
                Modifier
                    .offset(x = tile * t.x, y = tileHeight * t.y)
                    .width(tile * t.w)
                    .height(tileHeight * t.h),
            ) {
                Tile(
                    tile = t,
                    tileSize = tile,
                    active = active,
                    liveText = displayText(live),
                    series = SeriesWindow(series[watchChannel] ?: emptyList()),
                    items = TileItems(listItems(t, live)),
                    image = t.assetHash?.let { bitmaps[it] },
                    onPressStart = { vm.pressStart(board.id, t) },
                    onPressEnd = { vm.pressEnd(board.id, t) },
                    onSlider = { v -> vm.slider(board.id, t, v) },
                )
            }
        }
    }
}
