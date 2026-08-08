package com.ghazaleh.localcloud.ui

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.ghazaleh.localcloud.engine.Item
import com.ghazaleh.localcloud.files.OutgoingFiles
import com.ghazaleh.localcloud.service.SyncService
import com.ghazaleh.localcloud.ui.components.EmptyState
import com.ghazaleh.localcloud.ui.components.StatusDot
import com.ghazaleh.localcloud.ui.devices.DevicesScreen
import com.ghazaleh.localcloud.ui.files.FilesScreen
import com.ghazaleh.localcloud.ui.icons.LocalCloudIcons
import com.ghazaleh.localcloud.ui.trash.TrashScreen
import java.io.File

private enum class Tab(val label: String, val icon: ImageVector) {
    Files("Files", LocalCloudIcons.Files),
    Devices("Devices", LocalCloudIcons.Devices),
    Trash("Trash", LocalCloudIcons.Trash),
}

/**
 * Three destinations, because the engine has three ideas: items, devices, and
 * what is on its way out.
 *
 * Everything a person does lives on the screen where the thing itself is -
 * sending a file is on the file, unpairing is on the device, restoring is in
 * the trash. There is no settings screen because there is nothing yet to
 * configure that is not also a decision the engine makes better.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LocalCloudRoot(
    incoming: List<Uri> = emptyList(),
    onIncomingTaken: () -> Unit = {},
    viewModel: MainViewModel = viewModel(),
) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    val transfers by viewModel.transfers.collectAsStateWithLifecycle()
    val pairing by viewModel.pairing.collectAsStateWithLifecycle()
    val sharing by viewModel.sharing.collectAsStateWithLifecycle()
    val collision by viewModel.collision.collectAsStateWithLifecycle()
    val importing by viewModel.importing.collectAsStateWithLifecycle()
    val renaming by viewModel.renaming.collectAsStateWithLifecycle()
    val backgroundSync by viewModel.backgroundSync.collectAsStateWithLifecycle()

    var tab by rememberSaveable { mutableStateOf(Tab.Files) }
    val snackbarHostState = remember { SnackbarHostState() }
    val context = LocalContext.current

    // The service is kept to match the setting rather than being toggled
    // alongside it. Driving it from here also means it is only ever started
    // with a screen present, which is the one condition Android imposes on
    // starting a foreground service at all.
    LaunchedEffect(backgroundSync) {
        if (backgroundSync) SyncService.start(context) else SyncService.stop(context)
    }

    val askToNotify = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { granted ->
        if (granted) viewModel.setBackgroundSync(true) else viewModel.reportNotificationsRefused()
    }

    val onBackgroundSyncChange: (Boolean) -> Unit = { wanted ->
        val allowed = ContextCompat.checkSelfPermission(
            context,
            Manifest.permission.POST_NOTIFICATIONS,
        ) == PackageManager.PERMISSION_GRANTED

        when {
            !wanted -> viewModel.setBackgroundSync(false)
            allowed -> viewModel.setBackgroundSync(true)
            // Asked before anything starts, so the switch never lands on a
            // device that is syncing invisibly.
            else -> askToNotify.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }

    LaunchedEffect(Unit) {
        viewModel.notices.collect { notice ->
            snackbarHostState.showSnackbar(notice.text)
        }
    }

    val picker = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenDocument()
    ) { uri -> uri?.let(viewModel::importFrom) }

    // Files handed over by another app. Taken once, and the screen moves to
    // where they landed - a share that appeared to do nothing because the app
    // opened on a different tab would be indistinguishable from one that
    // failed.
    LaunchedEffect(incoming) {
        if (incoming.isNotEmpty()) {
            viewModel.importAll(incoming)
            tab = Tab.Files
            onIncomingTaken()
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text("LocalCloud", style = MaterialTheme.typography.titleLarge)
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(6.dp),
                        ) {
                            StatusDot(on = state.running)
                            Text(
                                text = when {
                                    state.fatal != null -> "Stopped"
                                    !state.ready -> "Starting…"
                                    state.running -> state.thisDevice.name.ifBlank { "This device" }
                                    else -> "Paused"
                                },
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                },
            )
        },
        bottomBar = {
            NavigationBar {
                Tab.entries.forEach { entry ->
                    NavigationBarItem(
                        selected = tab == entry,
                        onClick = { tab = entry },
                        icon = {
                            NavigationIcon(
                                icon = entry.icon,
                                label = entry.label,
                                marked = when (entry) {
                                    Tab.Devices -> state.offers.isNotEmpty()
                                    Tab.Files -> state.collisions.isNotEmpty()
                                    Tab.Trash -> false
                                },
                            )
                        },
                        label = { Text(entry.label) },
                    )
                }
            }
        },
        floatingActionButton = {
            if (tab == Tab.Files && state.fatal == null) {
                ExtendedFloatingActionButton(
                    onClick = { picker.launch(arrayOf("*/*")) },
                    icon = {
                        if (importing) {
                            CircularProgressIndicator(
                                modifier = Modifier.size(20.dp),
                                strokeWidth = 2.dp,
                                color = MaterialTheme.colorScheme.onPrimaryContainer,
                            )
                        } else {
                            Icon(LocalCloudIcons.Add, contentDescription = null)
                        }
                    },
                    text = { Text(if (importing) "Adding…" else "Add a file") },
                )
            }
        },
        snackbarHost = { SnackbarHost(snackbarHostState) },
    ) { innerPadding ->
        Box(modifier = Modifier.padding(innerPadding)) {
            val fatal = state.fatal
            when {
                fatal != null -> EmptyState(
                    icon = LocalCloudIcons.Warning,
                    title = "The engine could not start",
                    body = fatal,
                    modifier = Modifier.fillMaxSize(),
                )

                !state.ready -> Box(
                    modifier = Modifier.fillMaxSize(),
                    contentAlignment = Alignment.Center,
                ) { CircularProgressIndicator() }

                else -> when (tab) {
                    Tab.Files -> FilesScreen(
                        state = state,
                        transfers = transfers,
                        onOpen = openWith(context, viewModel, OutgoingFiles::viewIntent, "Open"),
                        onSendElsewhere = openWith(
                            context,
                            viewModel,
                            OutgoingFiles::sendIntent,
                            "Send",
                        ),
                        onShare = viewModel::beginSharing,
                        onPull = viewModel::pull,
                        onDeleteHere = viewModel::deleteHere,
                        onDeleteFrom = viewModel::deleteFrom,
                        onOpenCollision = viewModel::openCollision,
                        contentPadding = ContentInset,
                    )

                    Tab.Devices -> DevicesScreen(
                        state = state,
                        onPair = viewModel::startPairing,
                        onOpenOffer = viewModel::openOffer,
                        onUnpair = viewModel::unpair,
                        onRename = viewModel::beginRename,
                        backgroundSync = backgroundSync,
                        onBackgroundSyncChange = onBackgroundSyncChange,
                        contentPadding = ContentInset,
                    )

                    Tab.Trash -> TrashScreen(
                        state = state,
                        onRestore = viewModel::restore,
                        onDestroy = viewModel::destroy,
                        contentPadding = ContentInset,
                    )
                }
            }
        }
    }

    PairingDialog(
        flow = pairing,
        onType = viewModel::typeCode,
        onSubmit = viewModel::submitCode,
        onDismiss = viewModel::dismissPairing,
    )

    val item = sharing
    if (item != null) {
        val holderIds = item.holders.map { it.deviceId }.toSet()
        ShareDialog(
            item = item,
            candidates = state.paired.filter { it.id !in holderIds },
            onlineIds = state.visible.map { it.deviceId }.toSet(),
            onSend = { deviceIds -> viewModel.share(item.id, deviceIds) },
            onDismiss = viewModel::stopSharing,
        )
    }

    val draft = renaming
    if (draft != null) {
        RenameDialog(
            draft = draft,
            onType = viewModel::typeName,
            onSubmit = viewModel::submitRename,
            onDismiss = viewModel::dismissRename,
        )
    }

    val contested = collision
    if (contested != null) {
        CollisionDialog(
            collision = contested,
            onResolve = { resolution -> viewModel.resolveCollision(contested.id, resolution) },
            onDismiss = viewModel::dismissCollision,
        )
    }
}

/**
 * A navigation icon that can carry a mark.
 *
 * Hand-drawn rather than a badge component: the only thing it ever has to say
 * is "there is something here", and a dot says that without a number that would
 * imply the count matters.
 */
@Composable
private fun NavigationIcon(
    icon: ImageVector,
    label: String,
    marked: Boolean,
) {
    Box {
        Icon(imageVector = icon, contentDescription = label)
        if (marked) {
            Surface(
                shape = CircleShape,
                color = MaterialTheme.colorScheme.tertiary,
                modifier = Modifier
                    .size(8.dp)
                    .align(Alignment.TopEnd)
                    .offset(x = 3.dp, y = (-2).dp),
                content = {},
            )
        }
    }
}

/** Room for the floating button at the bottom of every list. */
private val ContentInset = PaddingValues(top = 8.dp, bottom = 96.dp)

/**
 * Hands a file to another app, whatever "hand" means for the intent given.
 *
 * Always through a chooser. This app has no business deciding which viewer or
 * which destination is the right one, and a chooser is also what makes the
 * no-app-can-do-this case a sentence on screen rather than an exception.
 */
private fun openWith(
    context: Context,
    viewModel: MainViewModel,
    intentFor: (Context, File) -> Intent,
    title: String,
): (Item) -> Unit = { item ->
    val path = item.localPath
    if (path == null) {
        viewModel.reportFileUnavailable(item.name)
    } else {
        runCatching {
            val file = File(path)
            check(file.exists()) { "the file is no longer on this device" }
            context.startActivity(
                Intent.createChooser(intentFor(context, file), "$title ${item.name}")
            )
        }.onFailure { viewModel.reportFileUnavailable(item.name) }
    }
}
