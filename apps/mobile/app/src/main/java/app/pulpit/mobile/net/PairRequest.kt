//! M8 Bluetooth-style pairing over HTTP (plan 014): the tablet posts a
//! request to a desktop it discovered over mDNS, shows the returned
//! verification code, and polls until the operator confirms - both
//! screens display the same number. Plain JSON REST; the WebSocket
//! pairing path only starts after the approval.

package app.pulpit.mobile.net

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody

@Serializable
data class PairRequestCreated(
    val request_id: String,
    val code: String,
    val expires_in_secs: Long = 0,
)

@Serializable
data class PairRequestStatus(
    val status: String,
)

@Serializable
private data class PairRequestName(val name: String)

/** One desktop found over mDNS (a resolved NsdServiceInfo). */
data class DiscoveredDesktop(
    val name: String,
    val host: String,
    val port: Int,
)

/** A refused request: 409 (one already live), loopback, browser-origin. */
class PairRequestRejected(message: String) : Exception(message)

private val json = Json { ignoreUnknownKeys = true }

suspend fun createPairRequest(
    http: OkHttpClient,
    host: String,
    port: Int,
    name: String,
): PairRequestCreated = withContext(Dispatchers.IO) {
    val payload = Json.encodeToString(PairRequestName.serializer(), PairRequestName(name))
    val request = Request.Builder()
        .url("http://$host:$port/v2/pair-request")
        .post(payload.toRequestBody("application/json".toMediaType()))
        .build()
    http.newCall(request).execute().use { resp ->
        val text = resp.body?.string().orEmpty()
        if (resp.code == 409) {
            throw PairRequestRejected("Na komputerze trwa już inne parowanie - spróbuj za chwilę")
        }
        if (!resp.isSuccessful) {
            throw PairRequestRejected("Komputer odrzucił żądanie (HTTP ${resp.code})")
        }
        json.decodeFromString(PairRequestCreated.serializer(), text)
    }
}

/** One poll; returns the decision word from the server
 *  (pending / approved / rejected / expired / unknown). */
suspend fun pairRequestStatus(http: OkHttpClient, host: String, port: Int, id: String): String =
    withContext(Dispatchers.IO) {
        val request = Request.Builder().url("http://$host:$port/v2/pair-request/$id").build()
        http.newCall(request).execute().use { resp ->
            if (!resp.isSuccessful) {
                return@use "unknown"
            }
            json.decodeFromString(PairRequestStatus.serializer(), resp.body?.string().orEmpty()).status
        }
    }
