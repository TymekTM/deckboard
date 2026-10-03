package app.pulpit.mobile.state

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Token-at-rest decisions: what a raw prefs entry means and what the
 *  ViewModel must do with it (ADR-008: Keystore-encrypted at rest,
 *  plaintext tokens migrate, unreadable tokens re-pair). */
class TokenVaultTest {

    private val fakeIv = ByteArray(12) { it.toByte() }
    private val fakeCiphertext = byteArrayOf(1, 2, 3, 4)

    /** A decrypt that never runs: plaintext must not touch the cipher. */
    private val neverDecrypt: (ByteArray, ByteArray) -> String? =
        { _, _ -> throw AssertionError("decrypt must not be called for plaintext") }

    @Test
    fun absentTokenNeedsNothing() {
        assertEquals(TokenLoad(null, rewrite = false), planTokenLoad(null, neverDecrypt))
        assertEquals(TokenLoad(null, rewrite = false), planTokenLoad("", neverDecrypt))
        assertEquals(TokenLoad(null, rewrite = false), planTokenLoad("   ", neverDecrypt))
    }

    @Test
    fun legacyPlaintextTokenMigrates() {
        val load = planTokenLoad("fake-legacy-token", neverDecrypt)
        assertEquals("fake-legacy-token", load.token)
        assertTrue(load.rewrite)
    }

    @Test
    fun encryptedTokenDecryptsWithoutRewrite() {
        val raw = encodeEnvelope(fakeIv, fakeCiphertext)
        val load = planTokenLoad(raw) { iv, data ->
            assertEquals(fakeIv.toList(), iv.toList())
            assertEquals(fakeCiphertext.toList(), data.toList())
            "fake-token"
        }
        assertEquals("fake-token", load.token)
        assertFalse(load.rewrite)
    }

    @Test
    fun unreadableCiphertextMeansScrubAndRePair() {
        val raw = encodeEnvelope(fakeIv, fakeCiphertext)
        val load = planTokenLoad(raw) { _, _ -> null }
        assertNull(load.token)
        assertTrue(load.rewrite)
    }

    @Test
    fun throwingDecryptMeansScrubAndRePair() {
        val raw = encodeEnvelope(fakeIv, fakeCiphertext)
        val load = planTokenLoad(raw) { _, _ -> error("keystore broken") }
        assertNull(load.token)
        assertTrue(load.rewrite)
    }

    @Test
    fun malformedEnvelopeMeansScrubAndRePair() {
        for (raw in listOf("enc1:", "enc1:zz:zz", "enc1:00", "enc1:00:00:00", "enc1:0:00")) {
            val load = planTokenLoad(raw) { _, _ -> "fake-token" }
            assertNull("raw=$raw", load.token)
            assertTrue("raw=$raw", load.rewrite)
        }
    }

    @Test
    fun envelopeRoundTrips() {
        val raw = encodeEnvelope(fakeIv, fakeCiphertext)
        assertTrue(raw.startsWith("enc1:"))
        val (iv, data) = decodeEnvelope(raw) ?: return assertNull("envelope decoded", null)
        assertEquals(fakeIv.toList(), iv.toList())
        assertEquals(fakeCiphertext.toList(), data.toList())
    }

    @Test
    fun decodeRejectsGarbage() {
        for (raw in listOf("", "tok", "enc1:", "enc1:zz:zz", "enc1:0:00", "enc:00:00")) {
            assertNull("raw=$raw", decodeEnvelope(raw))
        }
    }
}
