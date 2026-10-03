//! M8 discovery: the tablet's half of "both devices declare they are
//! open". Wraps Android NsdManager browsing for the desktop's mDNS
//! announcement (`_pulpit._tcp.`); every resolved instance becomes a
//! [DiscoveredDesktop]. NSD quirks on old ROMs are swallowed - discovery
//! is a convenience, manual pairing always works.

package app.pulpit.mobile.net

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo

class NsdDiscovery(
    context: Context,
    private val onFound: (DiscoveredDesktop) -> Unit,
) {
    private val nsdManager = context.getSystemService(NsdManager::class.java)
    private val resolved = mutableSetOf<String>()

    private val discoveryListener = object : NsdManager.DiscoveryListener {
        override fun onDiscoveryStarted(serviceType: String) {}
        override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {}
        override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {}
        override fun onDiscoveryStopped(serviceType: String) {}
        override fun onServiceLost(serviceInfo: NsdServiceInfo) {}

        override fun onServiceFound(serviceInfo: NsdServiceInfo) {
            if (!serviceInfo.serviceType.orEmpty().startsWith("_pulpit")) return
            nsdManager.resolveService(serviceInfo, object : NsdManager.ResolveListener {
                override fun onResolveFailed(info: NsdServiceInfo, errorCode: Int) {}
                override fun onServiceResolved(info: NsdServiceInfo) {
                    val host = info.host?.hostAddress ?: return
                    // the resolver can fire twice for one service
                    if (resolved.add("${info.serviceName}@$host")) {
                        onFound(DiscoveredDesktop(info.serviceName, host, info.port))
                    }
                }
            })
        }
    }

    fun start() {
        runCatching {
            nsdManager?.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, discoveryListener)
        }
    }

    fun stop() {
        runCatching { nsdManager?.stopServiceDiscovery(discoveryListener) }
    }

    companion object {
        const val SERVICE_TYPE = "_pulpit._tcp."
    }
}
