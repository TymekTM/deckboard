//! Application: initializes structured logging (Plog) before any
//! ViewModel can emit a line.

package app.pulpit.mobile

import android.app.Application
import app.pulpit.mobile.state.Plog

class PulpitApp : Application() {
    override fun onCreate() {
        super.onCreate()
        Plog.init(this)
    }
}
