package app.deckboard.mobile.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

/** The original Deckboard palette: near-black background, blue accents. */
object DeckColors {
    val background = Color(0xFF1B2A34)
    val boardBg = Color(0xFF22313C)
    val tileFallback = Color(0xFF34495E)
    val accent = Color(0xFF3E82F7)
}

private val DarkScheme = darkColorScheme(
    primary = DeckColors.accent,
    background = DeckColors.background,
    surface = DeckColors.boardBg,
    onPrimary = Color.White,
    onBackground = Color.White,
    onSurface = Color.White,
)

@Composable
fun DeckboardTheme(content: @Composable () -> Unit) {
    // the original app is dark-only
    isSystemInDarkTheme()
    MaterialTheme(colorScheme = DarkScheme, content = content)
}
