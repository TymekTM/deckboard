//! Address validation for the connect path (round 4, MOB-01): a host
//! OkHttp cannot parse used to escape as an IllegalArgumentException
//! from Request.Builder().url() straight through the click handler - and
//! since the bad host was already saved, as a launch crash loop on every
//! later start. The check uses the same parser OkHttp will use, so
//! whatever passes here cannot throw later.

package app.pulpit.mobile.net

import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

/** Why `host:port` cannot become a socket URL; null when it can.
 *  Port 0 stands for "no parseable number" from the text field.
 *  The check parses the `http` form of the URL because that is exactly
 *  what OkHttp does internally: Request.Builder.url() rewrites
 *  ws/wss to http/https before HttpUrl parses the authority. A path
 *  separator is rejected outright - OkHttp would slide it into the URL
 *  path and connect to the bare host on the default port. */
fun addressError(host: String, port: Int): String? = when {
    host.isBlank() -> "enter the PC address"
    port !in 1..65535 -> "port must be between 1 and 65535"
    host.contains('/') || "http://$host:$port/v2/ws".toHttpUrlOrNull() == null ->
        "\"$host\" is not a valid address"
    else -> null
}
