package io.github.jgalego.ephemeral

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * The model key, sealed by a key this phone will not hand out.
 *
 * The secret is encrypted with an AES key held in the Android keystore, which
 * is backed by hardware on most devices. The consequence worth knowing: the
 * sealing key never leaves the keystore and cannot be extracted by this app or
 * by anything that reads its files, so a copy of the preferences on its own is
 * not the credential.
 *
 * The engine is never given a path to any of this — it is handed the key in
 * memory, for the duration of a call, and reads no environment variable.
 */
internal object Credential {
    private const val KEYSTORE = "AndroidKeyStore"
    private const val ALIAS = "ephemeral.model-key"
    private const val PREFERENCES = "credential"
    private const val ENTRY = "sealed"
    private const val TAG_BITS = 128
    private const val NONCE_BYTES = 12

    /** Seals [key] and stores it. */
    fun save(context: Context, key: String) {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, sealingKey())

        val sealed = cipher.doFinal(key.toByteArray(Charsets.UTF_8))
        val carried = cipher.iv + sealed

        preferences(context)
            .edit()
            .putString(ENTRY, Base64.encodeToString(carried, Base64.NO_WRAP))
            .apply()
    }

    /** The stored key, or null if there is none or it can no longer be opened. */
    fun read(context: Context): String? {
        val stored = preferences(context).getString(ENTRY, null) ?: return null
        return try {
            val carried = Base64.decode(stored, Base64.NO_WRAP)
            if (carried.size <= NONCE_BYTES) return null

            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(
                Cipher.DECRYPT_MODE,
                sealingKey(),
                GCMParameterSpec(TAG_BITS, carried, 0, NONCE_BYTES),
            )
            String(
                cipher.doFinal(carried, NONCE_BYTES, carried.size - NONCE_BYTES),
                Charsets.UTF_8,
            )
        } catch (failure: Exception) {
            // A keystore entry can be invalidated by the OS — a lock-screen
            // change, a restore onto another device. Treating that as "no key"
            // asks for it again, which is recoverable; crashing is not.
            null
        }
    }

    /** Whether a key is stored at all. */
    fun present(context: Context): Boolean = read(context) != null

    /** Removes the key, and the sealing key with it. */
    fun forget(context: Context) {
        preferences(context).edit().remove(ENTRY).apply()
        try {
            keystore().deleteEntry(ALIAS)
        } catch (failure: Exception) {
            // Already gone is the outcome that was wanted.
        }
    }

    private fun preferences(context: Context) =
        context.applicationContext.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)

    private fun keystore(): KeyStore = KeyStore.getInstance(KEYSTORE).apply { load(null) }

    private fun sealingKey(): SecretKey {
        val existing = keystore().getKey(ALIAS, null) as? SecretKey
        if (existing != null) {
            return existing
        }

        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
        generator.init(
            KeyGenParameterSpec.Builder(
                ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                // Deliberately not requiring the screen to be unlocked for each
                // use: generation runs while the phone may be in a pocket, and
                // an authentication prompt in the middle of it would fail the
                // request rather than protect anything.
                .setUserAuthenticationRequired(false)
                .build(),
        )
        return generator.generateKey()
    }
}
