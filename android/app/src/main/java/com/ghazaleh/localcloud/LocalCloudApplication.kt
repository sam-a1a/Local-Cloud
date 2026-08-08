package com.ghazaleh.localcloud

import android.app.Application
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import com.ghazaleh.localcloud.engine.EngineHost
import com.ghazaleh.localcloud.engine.EngineRepository
import com.ghazaleh.localcloud.service.SyncNotification
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

/**
 * Holds the engine for as long as the process lives.
 *
 * It cannot live in an Activity or a ViewModel: the engine owns a database, a
 * block store and a tokio runtime, and rebuilding all of that because the phone
 * was rotated would be absurd. One per process, created here.
 *
 * **The engine runs while anything needs it to.** An open screen is one such
 * reason and is registered here; a foreground service is the other. They
 * overlap constantly - watching a transfer and then locking the phone - so the
 * repository counts reasons rather than being told to start and stop, and the
 * engine only goes down when the last one is withdrawn.
 */
class LocalCloudApplication : Application() {

    /**
     * Outlives every screen, so engine work started by one is not cancelled by
     * leaving it. [SupervisorJob] because one failed call should not take the
     * scope, and the rest of the app, with it.
     */
    private val engineScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    lateinit var repository: EngineRepository
        private set

    lateinit var preferences: Preferences
        private set

    override fun onCreate() {
        super.onCreate()

        preferences = Preferences(this)

        // Registered once, for the life of the install. Creating a channel that
        // already exists updates it rather than duplicating it, so this is also
        // how its wording stays current.
        SyncNotification.ensureChannel(this)

        repository = EngineRepository(EngineHost(this, preferences), engineScope)
        repository.begin()

        // An open screen is one reason to run the engine. It is no longer the
        // only one, so this says so rather than starting and stopping outright.
        ProcessLifecycleOwner.get().lifecycle.addObserver(
            object : DefaultLifecycleObserver {
                override fun onStart(owner: LifecycleOwner) {
                    engineNeededBy(EngineRepository.RunReason.Foreground)
                }

                override fun onStop(owner: LifecycleOwner) {
                    engineNoLongerNeededBy(EngineRepository.RunReason.Foreground)
                }
            }
        )
    }

    /**
     * Registers a reason on the application's scope.
     *
     * Exposed because the foreground service is the other caller, and its own
     * lifetime is exactly what must not bound this work: a scope cancelled in
     * `Service.onDestroy` would cancel the call that releases the engine, and
     * the engine would stay running with nothing left wanting it to.
     */
    fun engineNeededBy(reason: EngineRepository.RunReason) {
        engineScope.launch { repository.engineNeededBy(reason) }
    }

    fun engineNoLongerNeededBy(reason: EngineRepository.RunReason) {
        engineScope.launch { repository.engineNoLongerNeededBy(reason) }
    }
}
