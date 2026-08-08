package com.ghazaleh.localcloud

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.mutableStateOf
import com.ghazaleh.localcloud.ui.LocalCloudRoot
import com.ghazaleh.localcloud.ui.theme.LocalCloudTheme

/**
 * The only Activity.
 *
 * It deliberately owns nothing. The engine belongs to the process, not to a
 * screen, so rotating the phone or recreating this Activity does not stop
 * discovery, interrupt a transfer, or drop a pairing half way through.
 *
 * The one thing it does own is the intent that started it, because the share
 * sheet is a way into this app and only an Activity is told about it.
 */
class MainActivity : ComponentActivity() {

    /**
     * Files handed over by another app, waiting to be imported.
     *
     * State rather than a direct call, because the thing that knows how to
     * import is a ViewModel that does not exist yet when the intent arrives.
     * Cleared once taken, so a configuration change does not import twice.
     */
    private val shared = mutableStateOf<List<Uri>>(emptyList())

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()

        shared.value = filesIn(intent)

        setContent {
            LocalCloudTheme {
                LocalCloudRoot(
                    incoming = shared.value,
                    onIncomingTaken = { shared.value = emptyList() },
                )
            }
        }
    }

    /**
     * A second share while the app is already open.
     *
     * `singleTop` in the manifest is what routes it here instead of building a
     * second copy of the app on top of the first.
     */
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        shared.value = filesIn(intent)
    }

    private fun filesIn(intent: Intent?): List<Uri> = when (intent?.action) {
        Intent.ACTION_SEND ->
            listOfNotNull(intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java))

        Intent.ACTION_SEND_MULTIPLE ->
            intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java).orEmpty()

        else -> emptyList()
    }
}
