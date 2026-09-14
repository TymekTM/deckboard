package app.deckboard.mobile

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** The deck tablet starts the board itself after a power loss; the server
 *  address and pairing live in shared prefs, so launching is all it takes. */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action == Intent.ACTION_BOOT_COMPLETED) {
            context.startActivity(
                Intent(context, MainActivity::class.java)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        }
    }
}
