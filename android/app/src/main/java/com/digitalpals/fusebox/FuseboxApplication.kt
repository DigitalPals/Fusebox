package com.digitalpals.fusebox

import android.app.Application
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import com.digitalpals.fusebox.data.AppStore
import com.digitalpals.fusebox.data.AppUpdater
import com.digitalpals.fusebox.data.SecureStorage
import com.digitalpals.fusebox.data.SelfInstaller
import com.digitalpals.fusebox.data.UpdateWorker
import com.digitalpals.fusebox.push.FuseboxNotifications
import com.digitalpals.fusebox.push.PushManager
import com.digitalpals.fusebox.widget.FuseboxWidgetUpdater
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

class FuseboxApplication : Application() {
    lateinit var storage: SecureStorage
        private set
    lateinit var store: AppStore
        private set
    lateinit var pushManager: PushManager
        private set
    lateinit var updater: AppUpdater
        private set

    private val appScope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    override fun onCreate() {
        super.onCreate()
        FuseboxNotifications.ensureChannels(this)
        storage = SecureStorage(this)
        store = AppStore(this, storage)
        pushManager = PushManager(this, storage)
        updater = AppUpdater(this, storage)
        SelfInstaller.ensureChannel(this)
        UpdateWorker.schedule(this)
        FuseboxWidgetUpdater.schedulePeriodic(this)

        ProcessLifecycleOwner.get().lifecycle.addObserver(object : DefaultLifecycleObserver {
            override fun onStart(owner: LifecycleOwner) {
                store.resume()
                updater.checkOnForeground()
                appScope.launch { pushManager.onLaunchOrForeground(store.currentApi()) }
            }

            override fun onStop(owner: LifecycleOwner) {
                store.stop()
            }
        })
    }
}
