package com.digitalpals.fusebox.data

import android.content.Context
import com.digitalpals.fusebox.push.FuseboxNotifications
import com.digitalpals.fusebox.widget.FuseboxWidgetUpdater
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import java.time.Instant
import kotlin.math.min

enum class LiveState { CONNECTING, LIVE, OFFLINE }

data class AccountGroup(
    val providerKey: String,
    val name: String,
    val accounts: List<Account>,
)

data class AppUiState(
    val isConnected: Boolean = false,
    val serverUrl: String? = null,
    val serverHost: String? = null,
    val overview: Overview? = null,
    val accounts: List<Account> = emptyList(),
    val faults: List<Fault> = emptyList(),
    val dismissedFaults: List<Fault> = emptyList(),
    val requests: List<RequestLog> = emptyList(),
    val requestsFull: Boolean = false,
    val load: Map<String, AccountLoad> = emptyMap(),
    val activity: Map<String, AccountActivity> = emptyMap(),
    val live: LiveState = LiveState.CONNECTING,
    val loadError: String? = null,
    val lastLoaded: Instant? = null,
    val isRefreshing: Boolean = false,
    val quotaDisplay: QuotaDisplay = QuotaDisplay.standard,
    val outageAlertsEnabled: Boolean = true,
    val demoMode: Boolean = false,
) {
    val badgeCount: Int get() = faults.count { it.countsOnBadge }
    val weeklyPools: List<WeeklyPool> get() = WeeklyPool.weekly(overview, accounts)
    val subscriptions: List<Account> get() = accounts.filter { it.metered && !it.disabled }
    val otherCircuits: List<Account> get() = accounts.filter { !it.metered && !it.disabled }

    fun account(id: String?): Account? = if (id == null) null else accounts.firstOrNull { it.id == id }
    fun inFlight(id: String): Int = load[id]?.inFlight ?: 0

    val groups: List<AccountGroup>
        get() {
            val order = mutableListOf<String>()
            val map = linkedMapOf<String, MutableList<Account>>()
            for (a in accounts) {
                val key = if (a.provider == "openai-compat") "compat:${a.group ?: ""}" else a.provider
                if (key !in map) order.add(key)
                map.getOrPut(key) { mutableListOf() }.add(a)
            }
            return order.map { k ->
                val list = map[k]!!
                AccountGroup(providerKey = k, name = list[0].providerName, accounts = list)
            }
        }

    /** The last 60 minutes with empty minutes filled in, oldest first. */
    fun lastHour(now: Instant = Instant.now()): List<MinuteBucket> {
        val nowMin = now.epochSecond / 60
        val byMin = overview?.series.orEmpty().associateBy { it.minute }
        return ((nowMin - 59)..nowMin).map { m -> byMin[m] ?: MinuteBucket(minute = m) }
    }
}

/**
 * Single source of truth for a Fusebox server connection, living in the Application instance so
 * folding/unfolding, screen rotations, and multi-window resizes never drop state or reconnect the socket.
 */
class AppStore(
    private val context: Context,
    private val storage: SecureStorage,
) {
    private val appContext = context.applicationContext
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    private var api: FuseboxApi? = null
    private var socket: WebSocket? = null
    private var reconnectJob: Job? = null
    private var pollJob: Job? = null
    private var refreshJob: Job? = null
    private var accountsRefreshJob: Job? = null
    private var retryDelaySec: Long = 1L
    private var generation: Int = 0
    private var widgetReloadedMs: Long = 0L
    private var lastFaultKeys: List<String> = emptyList()

    private val _state = MutableStateFlow(
        AppUiState(
            quotaDisplay = storage.quotaDisplay,
            outageAlertsEnabled = storage.outageAlertsEnabled,
        ),
    )
    val state: StateFlow<AppUiState> = _state.asStateFlow()

    init {
        val savedUrl = storage.serverUrl
        val savedKey = storage.managementKey
        if (!savedUrl.isNullOrBlank() && !savedKey.isNullOrBlank()) {
            api = FuseboxApi(savedUrl, savedKey)
            _state.update {
                it.copy(
                    isConnected = true,
                    serverUrl = savedUrl,
                    serverHost = api?.host,
                )
            }
        }
    }

    fun currentApi(): FuseboxApi? = api

    fun setQuotaDisplay(mode: QuotaDisplay) {
        if (_state.value.quotaDisplay == mode) return
        storage.quotaDisplay = mode
        _state.update { it.copy(quotaDisplay = mode) }
        FuseboxWidgetUpdater.updateAll(appContext)
    }

    fun setOutageAlertsEnabled(enabled: Boolean) {
        storage.outageAlertsEnabled = enabled
        _state.update { it.copy(outageAlertsEnabled = enabled) }
        FuseboxNotifications.syncOutages(appContext, _state.value.faults, enabled)
    }

    fun connect(url: String, key: String) {
        val prevUrl = storage.serverUrl
        stop()
        if (prevUrl != url) FuseboxNotifications.endAllOutages(appContext)
        storage.serverUrl = url
        storage.managementKey = key
        api = FuseboxApi(url, key)
        _state.update {
            it.copy(
                isConnected = true,
                demoMode = false,
                serverUrl = url,
                serverHost = api?.host,
                loadError = null,
            )
        }
        start()
    }

    fun signOut() {
        stop()
        FuseboxNotifications.endAllOutages(appContext)
        storage.managementKey = null
        api = null
        lastFaultKeys = emptyList()
        _state.update {
            AppUiState(
                isConnected = false,
                serverUrl = storage.serverUrl,
                quotaDisplay = it.quotaDisplay,
                outageAlertsEnabled = it.outageAlertsEnabled,
            )
        }
        storage.saveWidgetSnapshot(WidgetSnapshot())
        FuseboxWidgetUpdater.updateAll(appContext)
    }

    fun start() {
        if (api == null || _state.value.demoMode) return
        refreshJob?.cancel()
        refreshJob = scope.launch { refreshAll() }
        connectLive()
    }

    fun stop() {
        generation += 1
        reconnectJob?.cancel(); reconnectJob = null
        socket?.close(1000, "closed"); socket = null
        pollJob?.cancel(); pollJob = null
        refreshJob?.cancel(); refreshJob = null
        accountsRefreshJob?.cancel(); accountsRefreshJob = null
    }

    fun resume() {
        if (api == null || _state.value.demoMode) return
        retryDelaySec = 1L
        stop()
        start()
    }

    suspend fun refreshAll() {
        val currentApi = api ?: return
        val gen = generation
        _state.update { it.copy(isRefreshing = true) }
        try {
            coroutineScope {
                val oDef = async { currentApi.overview() }
                val aDef = async { currentApi.accounts() }
                val fDef = async { currentApi.faults() }
                val rDef = async { currentApi.requests(limit = if (_state.value.requestsFull) null else 60) }
                val ov = oDef.await()
                val ac = aDef.await()
                val fa = fDef.await()
                val rq = rDef.await()
                if (gen != generation) return@coroutineScope
                val dismissed = runCatching { currentApi.dismissedFaults() }.getOrNull()
                if (gen != generation) return@coroutineScope
                _state.update {
                    it.copy(
                        overview = ov,
                        accounts = ac,
                        requests = rq,
                        loadError = null,
                        lastLoaded = Instant.now(),
                    )
                }
                setFaults(fa, dismissed)
                dataChanged()
            }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            if (gen == generation) {
                _state.update { it.copy(loadError = e.message ?: "Can’t reach Fusebox") }
            }
        } finally {
            if (gen == generation) _state.update { it.copy(isRefreshing = false) }
        }
    }

    suspend fun loadFullRequests() {
        val currentApi = api ?: return
        if (_state.value.requestsFull) return
        val gen = generation
        val all = runCatching { currentApi.requests(limit = null) }.getOrNull() ?: return
        if (gen == generation) {
            _state.update { it.copy(requests = all, requestsFull = true) }
        }
    }

    fun refreshAccounts() {
        val currentApi = api ?: return
        val gen = generation
        accountsRefreshJob?.cancel()
        accountsRefreshJob = scope.launch {
            delay(250)
            if (!isActive || gen != generation) return@launch
            val ac = runCatching { currentApi.accounts() }.getOrNull()
            if (ac != null && gen == generation) _state.update { it.copy(accounts = ac) }
            if (gen != generation) return@launch
            val ov = runCatching { currentApi.overview() }.getOrNull()
            if (ov != null && gen == generation) _state.update { it.copy(overview = ov) }
            if (gen == generation) dataChanged()
        }
    }

    suspend fun loadActivity(id: String) {
        val currentApi = api ?: return
        val gen = generation
        val act = runCatching { currentApi.activity(id) }.getOrNull() ?: return
        if (gen == generation) {
            _state.update { it.copy(activity = it.activity + (id to act)) }
        }
    }

    // MARK: - Live WebSocket

    private fun connectLive() {
        val currentApi = api ?: return
        val gen = generation
        _state.update { it.copy(live = LiveState.CONNECTING) }
        socket = currentApi.openLiveSocket(object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                scope.launch {
                    if (gen != generation) {
                        webSocket.close(1000, null)
                        return@launch
                    }
                    retryDelaySec = 1L
                    pollJob?.cancel(); pollJob = null
                    _state.update { it.copy(live = LiveState.LIVE) }
                }
            }

            override fun onMessage(webSocket: WebSocket, text: String) {
                scope.launch {
                    if (gen != generation) return@launch
                    val msg = runCatching { LiveMessage.decode(text) }.getOrNull() ?: return@launch
                    onLiveMessage(msg)
                }
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                scope.launch {
                    if (gen != generation) return@launch
                    onSocketClosed(gen)
                }
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                scope.launch {
                    if (gen != generation) return@launch
                    onSocketClosed(gen)
                }
            }
        })
    }

    private fun onSocketClosed(gen: Int) {
        if (gen != generation) return
        socket = null
        _state.update { it.copy(live = LiveState.OFFLINE, load = emptyMap()) }
        startPolling(gen)
        val waitSec = retryDelaySec
        retryDelaySec = min(retryDelaySec * 2, 15L)
        reconnectJob?.cancel()
        reconnectJob = scope.launch {
            delay(waitSec * 1000L)
            if (isActive && gen == generation) connectLive()
        }
    }

    private fun startPolling(gen: Int) {
        if (pollJob?.isActive == true) return
        pollJob = scope.launch {
            while (isActive && gen == generation && _state.value.live != LiveState.LIVE) {
                delay(30_000L)
                if (isActive && gen == generation && _state.value.live != LiveState.LIVE) {
                    refreshAll()
                }
            }
        }
    }

    private fun onLiveMessage(msg: LiveMessage) {
        when (msg) {
            is LiveMessage.Request -> onRequest(msg.log)
            is LiveMessage.Load -> _state.update { it.copy(load = msg.load) }
            is LiveMessage.Faults -> setFaults(msg.faults, msg.dismissed)
            is LiveMessage.Tick -> _state.update { st ->
                val ov = st.overview ?: return@update st
                st.copy(
                    overview = ov.copy(
                        totals = msg.tick.totals ?: ov.totals,
                        active = msg.tick.active ?: ov.active,
                    ),
                )
            }
            LiveMessage.Accounts -> refreshAccounts()
            LiveMessage.Models, is LiveMessage.Other -> Unit
        }
    }

    private fun onRequest(r: RequestLog) {
        _state.update { st ->
            val updatedReqs = (listOf(r) + st.requests).take(300)
            val ov = st.overview ?: return@update st.copy(requests = updatedReqs)
            val minute = r.ts.epochSecond / 60
            val cutoff = Instant.now().epochSecond / 60 - 59
            val series = ov.series.orEmpty().filter { it.minute >= cutoff }.toMutableList()
            if (minute >= cutoff) {
                val idx = series.indexOfFirst { it.minute == minute }
                val tok = (r.inputTokens ?: 0L) + (r.outputTokens ?: 0L) + (r.cacheTokens ?: 0L)
                if (idx >= 0) {
                    val cur = series[idx]
                    series[idx] = cur.copy(
                        requests = cur.requests + 1,
                        failed = (cur.failed ?: 0) + if (r.isError) 1 else 0,
                        tokens = (cur.tokens ?: 0L) + tok,
                    )
                } else {
                    series.add(
                        MinuteBucket(
                            minute = minute,
                            requests = 1,
                            failed = if (r.isError) 1 else 0,
                            tokens = tok,
                        ),
                    )
                    series.sortBy { it.minute }
                }
            }
            st.copy(requests = updatedReqs, overview = ov.copy(series = series))
        }
    }

    private fun setFaults(incoming: List<Fault>, dismissed: List<Fault>? = null) {
        fun rank(f: Fault): Int = when {
            f.kind == "pace" -> 2
            f.level == "err" -> 0
            else -> 1
        }
        val sorted = incoming.withIndex()
            .sortedWith(compareBy<IndexedValue<Fault>> { rank(it.value) }.thenBy { it.index })
            .map { it.value }
        _state.update {
            it.copy(
                faults = sorted,
                dismissedFaults = dismissed ?: it.dismissedFaults,
            )
        }
        val keys = sorted.map { it.key }
        if (keys != lastFaultKeys) {
            lastFaultKeys = keys
            reloadWidgets(force = true)
        }
        FuseboxNotifications.syncOutages(appContext, sorted, _state.value.outageAlertsEnabled)
    }

    private fun dataChanged() {
        val st = _state.value
        storage.saveWidgetSnapshot(WidgetSnapshot(st.overview, st.accounts, st.faults))
        reloadWidgets(force = false)
    }

    private fun reloadWidgets(force: Boolean) {
        val now = System.currentTimeMillis()
        if (!force && now - widgetReloadedMs < 300_000L) return
        widgetReloadedMs = now
        val st = _state.value
        storage.saveWidgetSnapshot(WidgetSnapshot(st.overview, st.accounts, st.faults))
        FuseboxWidgetUpdater.updateAll(appContext)
    }

    // MARK: - Fault actions

    @Serializable
    private data class KeyBody(val key: String)

    @Serializable
    private class EmptyBody

    suspend fun dismiss(fault: Fault) {
        val currentApi = api ?: return
        val gen = generation
        val st = _state.value
        setFaults(
            st.faults.filter { it.key != fault.key },
            st.dismissedFaults.filter { it.key != fault.key } + fault,
        )
        try {
            currentApi.send("POST", "/faults/dismiss", KeyBody.serializer(), KeyBody(fault.key))
        } catch (e: ApiException) {
            if (e.status != 404) {
                if (gen == generation) refreshAll()
                throw e
            }
        } catch (e: Exception) {
            if (gen == generation) refreshAll()
            throw e
        }
    }

    suspend fun restoreDismissedFaults() {
        val currentApi = api ?: return
        val gen = generation
        currentApi.send("POST", "/faults/restore", EmptyBody.serializer(), EmptyBody())
        if (gen == generation) {
            val st = _state.value
            setFaults(st.faults + st.dismissedFaults, emptyList())
        }
    }

    // MARK: - Account actions

    @Serializable
    private data class ToggleBody(val disabled: Boolean)

    @Serializable
    private data class ClearErrorBody(@SerialName("error_id") val errorId: String)

    @Serializable
    private data class PriorityBody(val priority: Int)

    @Serializable
    private data class RedeemBody(
        val action: String = "redeem",
        @SerialName("request_id") val requestId: String,
        @SerialName("grant_id") val grantId: String,
        val confirmed: Boolean = true,
    )

    suspend fun refreshQuota(id: String) {
        val currentApi = api ?: return
        val gen = generation
        val raw = currentApi.request("POST", "/accounts/${ConnectionHelper.encodePathSegment(id)}/quota/refresh")
        if (gen == generation) {
            val banked = runCatching { FuseboxJson.decodeFromString(BankedResets.serializer(), raw) }.getOrNull()
            if (banked?.checkedAt != null) {
                _state.update { st ->
                    st.copy(accounts = st.accounts.map { if (it.id == id) it.copy(bankedResets = banked) else it })
                }
            }
            refreshAccounts()
        }
    }

    suspend fun setDisabled(id: String, disabled: Boolean) {
        val currentApi = api ?: return
        val gen = generation
        currentApi.send("POST", "/accounts/${ConnectionHelper.encodePathSegment(id)}/toggle", ToggleBody.serializer(), ToggleBody(disabled))
        if (gen == generation) {
            _state.update { st ->
                st.copy(accounts = st.accounts.map { if (it.id == id) it.copy(disabled = disabled) else it })
            }
            refreshAccounts()
        }
    }

    suspend fun clearCooldowns(id: String) {
        val currentApi = api ?: return
        val gen = generation
        currentApi.request("POST", "/accounts/${ConnectionHelper.encodePathSegment(id)}/reset")
        if (gen == generation) refreshAccounts()
    }

    suspend fun clearError(id: String, errorId: String) {
        val currentApi = api ?: return
        val gen = generation
        currentApi.send(
            "POST",
            "/accounts/${ConnectionHelper.encodePathSegment(id)}/error/clear",
            ClearErrorBody.serializer(),
            ClearErrorBody(errorId),
        )
        if (gen == generation) refreshAccounts()
    }

    suspend fun setPriority(id: String, priority: Int) {
        val currentApi = api ?: return
        val gen = generation
        currentApi.send(
            "POST",
            "/accounts/${ConnectionHelper.encodePathSegment(id)}/priority",
            PriorityBody.serializer(),
            PriorityBody(priority),
        )
        if (gen == generation) {
            _state.update { st ->
                st.copy(accounts = st.accounts.map { if (it.id == id) it.copy(priority = priority) else it })
            }
            refreshAccounts()
        }
    }

    suspend fun bankedQuote(id: String): BankedResets {
        val currentApi = api ?: throw ApiException(0, "Not connected")
        val gen = generation
        val r = currentApi.get("/accounts/${ConnectionHelper.encodePathSegment(id)}/banked-resets", BankedResets.serializer())
        if (gen == generation) {
            _state.update { st ->
                st.copy(accounts = st.accounts.map { if (it.id == id) it.copy(bankedResets = r) else it })
            }
        }
        return r
    }

    suspend fun applyBankedReset(id: String, requestId: String, grantId: String) {
        val currentApi = api ?: return
        val gen = generation
        val raw = currentApi.send(
            "POST",
            "/accounts/${ConnectionHelper.encodePathSegment(id)}/banked-resets",
            RedeemBody.serializer(),
            RedeemBody(requestId = requestId, grantId = grantId),
        )
        if (gen == generation) {
            val banked = runCatching { FuseboxJson.decodeFromString(BankedResets.serializer(), raw) }.getOrNull()
            if (banked != null) {
                _state.update { st ->
                    st.copy(accounts = st.accounts.map { if (it.id == id) it.copy(bankedResets = banked) else it })
                }
            }
            refreshAccounts()
        }
    }

    /** Loads synthetic demo data into the store (Debug builds only, e.g. for foldable UI verification). */
    fun loadDemoData(
        overview: Overview,
        accounts: List<Account>,
        faults: List<Fault>,
        requests: List<RequestLog>,
        activity: Map<String, AccountActivity> = emptyMap(),
    ) {
        stop()
        api = null
        _state.update {
            it.copy(
                isConnected = true,
                demoMode = true,
                serverUrl = "https://fusebox.demo.ts.net",
                serverHost = "fusebox.demo.ts.net",
                overview = overview,
                accounts = accounts.map { a -> a.normalized() },
                faults = faults,
                requests = requests,
                requestsFull = true,
                activity = activity,
                live = LiveState.LIVE,
                loadError = null,
                lastLoaded = Instant.now(),
            )
        }
        storage.saveWidgetSnapshot(WidgetSnapshot(overview, accounts, faults))
    }
}
