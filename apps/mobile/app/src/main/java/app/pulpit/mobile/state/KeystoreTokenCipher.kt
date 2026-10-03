//! Device-only half of token-at-rest storage (ADR-008): AES-256-GCM with
//! a non-exportable AndroidKeyStore key, a fresh random IV per encrypt.
//! The decision logic around it (migration, unreadable-token handling)
//! is pure and unit-tested in TokenVault; this class needs a running
//! Android system, so it is exercised only on-device/instrumented.

package app.pulpit.mobile.state

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Log
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Encrypts/decrypts the pairing token with the `AndroidKeyStore` key
 *  [KEY_ALIAS], creating it on first use. Every method returns null on
 *  any failure - callers treat that as "no usable token", never crash. */
class KeystoreTokenCipher {

    /** Returns (iv, ciphertext), or null when the keystore is broken. */
    fun encrypt(plain: String): Pair<ByteArray, ByteArray>? = runCatching {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        // Without explicit params AndroidKeyStore generates a fresh
        // random IV per encryption and hands it back here.
        val iv = cipher.iv
        val data = cipher.doFinal(plain.toByteArray(Charsets.UTF_8))
        iv to data
    }.onFailure { warn("encrypt", it) }.getOrNull()

    /** Inverse of [encrypt]; null on tampering, key loss, or any error. */
    fun decrypt(iv: ByteArray, data: ByteArray): String? = runCatching {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(GCM_TAG_BITS, iv))
        String(cipher.doFinal(data), Charsets.UTF_8)
    }.onFailure { warn("decrypt", it) }.getOrNull()

    private fun key(): SecretKey {
        val keystore = KeyStore.getInstance(PROVIDER).apply { load(null) }
        (keystore.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, PROVIDER)
        generator.init(
            KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return generator.generateKey()
    }

    /** Class name only: exception text must never quote key material. */
    private fun warn(what: String, error: Throwable) {
        Log.w(TAG, "token vault: $what failed (${error.javaClass.simpleName})")
    }

    private companion object {
        const val TAG = "KeystoreTokenCipher"
        const val PROVIDER = "AndroidKeyStore"
        const val KEY_ALIAS = "pulpit-pairing-token"
        const val TRANSFORMATION = "AES/GCM/NoPadding"

        /** GCM auth tag length in bits, as GCMParameterSpec wants it. */
        const val GCM_TAG_BITS = 128
    }
}
