//! Pairing-token storage decisions (ADR-008): the token lives in the
//! usual SharedPreferences file, but only as an `enc1:` envelope -
//! AES-256-GCM ciphertext from an AndroidKeyStore key, with the random
//! IV framed alongside it. This file holds the Android-free decision
//! logic (unit-tested); the Keystore cipher itself is device-only
//! (see KeystoreTokenCipher).

package app.pulpit.mobile.state

/** What a raw "token" prefs entry turns out to be, and what must happen
 *  to the stored value afterwards. */
data class TokenLoad(
    /** Usable token, or null when the device must pair again. */
    val token: String?,
    /** True when the prefs entry must be replaced (migration from
     *  plaintext) or removed (unreadable garbage). */
    val rewrite: Boolean,
)

/** Envelope marker + version. Not a token value, safe to match. */
private const val PREFIX = "enc1:"

/** GCM's standard 96-bit IV; AndroidKeyStore generates one per encrypt. */
private const val IV_BYTES = 12

/** Interpret a raw prefs value. [decrypt] receives the framed IV and
 *  ciphertext and returns the token, or null when decryption fails.
 *  Never throws; a value that cannot be understood at all reads as
 *  "no token, scrub the entry" so the device re-pairs instead of
 *  crashing or looping on the same bad ciphertext. */
fun planTokenLoad(raw: String?, decrypt: (ByteArray, ByteArray) -> String?): TokenLoad {
    if (raw.isNullOrBlank()) return TokenLoad(token = null, rewrite = false)
    if (!raw.startsWith(PREFIX)) {
        // Pre-ADR-008 install: plaintext token, still valid - migrate.
        return TokenLoad(token = raw, rewrite = true)
    }
    val framed = decodeEnvelope(raw)
        ?: return TokenLoad(token = null, rewrite = true)
    val (iv, data) = framed
    val token = runCatching { decrypt(iv, data) }.getOrNull()
        ?: return TokenLoad(token = null, rewrite = true)
    if (token.isBlank()) return TokenLoad(token = null, rewrite = true)
    return TokenLoad(token = token, rewrite = false)
}

/** Frame IV + ciphertext as `enc1:<hex iv>:<hex ciphertext>`. */
fun encodeEnvelope(iv: ByteArray, data: ByteArray): String =
    PREFIX + hex(iv) + ":" + hex(data)

/** Inverse of [encodeEnvelope]; null when the framing is not intact. */
fun decodeEnvelope(raw: String): Pair<ByteArray, ByteArray>? {
    val parts = raw.split(":")
    if (parts.size != 3 || parts[0] != "enc1") return null
    val iv = unhex(parts[1]) ?: return null
    val data = unhex(parts[2]) ?: return null
    if (iv.size != IV_BYTES || data.isEmpty()) return null
    return iv to data
}

private fun hex(bytes: ByteArray): String {
    val digits = CharArray(bytes.size * 2)
    for (i in bytes.indices) {
        val b = bytes[i].toInt()
        digits[i * 2] = "0123456789abcdef"[(b shr 4) and 0xf]
        digits[i * 2 + 1] = "0123456789abcdef"[b and 0xf]
    }
    return String(digits)
}

private fun unhex(s: String): ByteArray? {
    if (s.isEmpty() || s.length % 2 != 0) return null
    val out = ByteArray(s.length / 2)
    for (i in out.indices) {
        val hi = Character.digit(s[i * 2], 16)
        val lo = Character.digit(s[i * 2 + 1], 16)
        if (hi < 0 || lo < 0) return null
        out[i] = ((hi shl 4) or lo).toByte()
    }
    return out
}
