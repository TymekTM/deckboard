//! Unit tests for pure haptic preferences mapping, intensity scaling and constants resolution.

package app.pulpit.mobile.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

class HapticsTest {

    @Test
    fun intensityMappingFromPolishStrings() {
        assertEquals(HapticIntensity.Light, HapticIntensity.fromString("Lekkie"))
        assertEquals(HapticIntensity.Light, HapticIntensity.fromString("lekkie"))
        assertEquals(HapticIntensity.Normal, HapticIntensity.fromString("Normalne"))
        assertEquals(HapticIntensity.Normal, HapticIntensity.fromString("normalne"))
        assertEquals(HapticIntensity.Strong, HapticIntensity.fromString("Mocne"))
        assertEquals(HapticIntensity.Strong, HapticIntensity.fromString("mocne"))
    }

    @Test
    fun intensityMappingFromEnglishStrings() {
        assertEquals(HapticIntensity.Light, HapticIntensity.fromString("Light"))
        assertEquals(HapticIntensity.Normal, HapticIntensity.fromString("Normal"))
        assertEquals(HapticIntensity.Strong, HapticIntensity.fromString("Strong"))
    }

    @Test
    fun intensityMappingFallbackOnInvalidOrNull() {
        assertEquals(HapticIntensity.Normal, HapticIntensity.fromString(null))
        assertEquals(HapticIntensity.Normal, HapticIntensity.fromString(""))
        assertEquals(HapticIntensity.Normal, HapticIntensity.fromString("unknown"))
        assertEquals(HapticIntensity.Strong, HapticIntensity.fromString("invalid", default = HapticIntensity.Strong))
    }

    @Test
    fun configFromPreferences() {
        val config = HapticConfig.fromPreferences(enabled = true, intensityStr = "Mocne")
        assertTrue(config.enabled)
        assertEquals(HapticIntensity.Strong, config.intensity)

        val disabled = HapticConfig.fromPreferences(enabled = false, intensityStr = "lekkie")
        assertEquals(false, disabled.enabled)
        assertEquals(HapticIntensity.Light, disabled.intensity)

        val defaultCfg = HapticConfig.fromPreferences(enabled = null, intensityStr = null)
        assertTrue(defaultCfg.enabled)
        assertEquals(HapticIntensity.Normal, defaultCfg.intensity)
    }

    @Test
    fun fallbackVibrationParamsScalesWithIntensity() {
        for (kind in HapticKind.values()) {
            val (lightDur, lightAmp) = fallbackVibrationParams(kind, HapticIntensity.Light)
            val (normalDur, normalAmp) = fallbackVibrationParams(kind, HapticIntensity.Normal)
            val (strongDur, strongAmp) = fallbackVibrationParams(kind, HapticIntensity.Strong)

            // Durations must grow with intensity
            assertTrue(lightDur < normalDur)
            assertTrue(normalDur < strongDur)

            // Amplitudes must be in valid byte range 1..255 and grow with intensity
            assertTrue(lightAmp in 1..255)
            assertTrue(normalAmp in 1..255)
            assertTrue(strongAmp in 1..255)
            assertTrue(lightAmp < normalAmp)
            assertTrue(normalAmp < strongAmp)
        }
    }

    @Test
    fun heavyDurationIsLargerThanClick() {
        val (clickDur, _) = fallbackVibrationParams(HapticKind.Click, HapticIntensity.Normal)
        val (heavyDur, _) = fallbackVibrationParams(HapticKind.Heavy, HapticIntensity.Normal)
        assertTrue("Heavy gesture feedback must be longer/heavier than short click", heavyDur > clickDur)
    }

    @Test
    fun sliderTickDurationIsShortest() {
        val (sliderDur, _) = fallbackVibrationParams(HapticKind.SliderTick, HapticIntensity.Light)
        val (clickDur, _) = fallbackVibrationParams(HapticKind.Click, HapticIntensity.Light)
        assertTrue("Slider tick duration should be shorter or equal to click", sliderDur <= clickDur)
    }

    @Test
    fun constantResolutionReturnsValidIntegers() {
        for (kind in HapticKind.values()) {
            for (intensity in HapticIntensity.values()) {
                val constant = resolveHapticFeedbackConstant(kind, intensity)
                assertNotNull("Feedback constant for $kind/$intensity must not be null", constant)
            }
        }
    }
}