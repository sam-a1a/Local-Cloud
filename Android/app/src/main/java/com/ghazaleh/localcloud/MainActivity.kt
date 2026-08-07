package com.ghazaleh.localcloud

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import com.ghazaleh.localcloud.ui.LocalCloudRoot
import com.ghazaleh.localcloud.ui.theme.LocalCloudTheme

/**
 * The only Activity.
 *
 * It deliberately owns nothing. The engine belongs to the process, not to a
 * screen, so rotating the phone or recreating this Activity does not stop
 * discovery, interrupt a transfer, or drop a pairing half way through.
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            LocalCloudTheme {
                LocalCloudRoot()
            }
        }
    }
}
