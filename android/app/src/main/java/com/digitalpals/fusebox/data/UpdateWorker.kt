package com.digitalpals.fusebox.data

import android.content.Context
import android.util.Log
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import com.digitalpals.fusebox.FuseboxApplication
import java.util.concurrent.TimeUnit

/** Every 6 hours, with a network: finds, downloads and installs a newer Fusebox by itself. */
class UpdateWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val updater = (applicationContext as? FuseboxApplication)?.updater ?: return Result.success()
        val outcome = updater.autoUpdate()
        Log.i("FuseboxUpdate", "Background update check: $outcome")
        return Result.success()
    }

    companion object {
        private const val NAME = "fusebox-app-update"

        fun schedule(context: Context) {
            if (!AppUpdater.selfUpdateSupported(context)) return
            val request = PeriodicWorkRequestBuilder<UpdateWorker>(6, TimeUnit.HOURS)
                .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
                .build()
            WorkManager.getInstance(context.applicationContext)
                .enqueueUniquePeriodicWork(NAME, ExistingPeriodicWorkPolicy.KEEP, request)
        }
    }
}
