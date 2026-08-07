package com.ghazaleh.localcloud

import android.app.Application
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import com.ghazaleh.localcloud.engine.EngineHost
import com.ghazaleh.localcloud.engine.EngineRepository
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
 * **The engine runs while the app is in the foreground and not otherwise.** It
 * is bound to the process lifecycle, so it comes up when any screen is visible
 * and goes down when the last one leaves. That is a real limitation and worth
 * being clear about: with the app closed, this device is invisible to the mesh
 * and nothing arrives. A foreground service would change that, and is the next
 * decision rather than an oversight - the app is deliberately being proven on
 * real hardware first, where discovery working at all is still an assumption.
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

    override fun onCreate() {
        super.onCreate()

        repository = EngineRepository(EngineHost(this), engineScope)
        repository.begin()

        ProcessLifecycleOwner.get().lifecycle.addObserver(
            object : DefaultLifecycleObserver {
                override fun onStart(owner: LifecycleOwner) {
                    engineScope.launch { repository.resume() }
                }

                override fun onStop(owner: LifecycleOwner) {
                    engineScope.launch { repository.pause() }
                }
            }
        )
    }
}
