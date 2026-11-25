package com.digitalpals.fusebox.data

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.DeserializationStrategy
import kotlinx.serialization.KSerializer
import kotlinx.serialization.Serializable
import kotlinx.serialization.builtins.ListSerializer
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import java.io.IOException
import java.net.ConnectException
import java.net.SocketTimeoutException
import java.net.URLEncoder
import java.net.UnknownHostException
import java.nio.charset.StandardCharsets
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLException

class ApiException(val status: Int, override val message: String) : Exception(message) {
    val isUnauthorized: Boolean get() = status == 401
}

object ConnectionHelper {
    /** Accepts "fusebox.example.ts.net" or a full URL; keeps scheme, host, port and path. */
    fun normalize(text: String): String? {
        var t = text.trim()
        if (t.isEmpty()) return null
        if (!t.contains("://")) t = "https://$t"
        while (t.endsWith("/")) t = t.dropLast(1)
        val parsed = t.toHttpUrlOrNull() ?: return null
        if (parsed.scheme !in setOf("https", "http") || parsed.host.isBlank()) return null
        return t
    }

    fun encodePathSegment(id: String): String =
        URLEncoder.encode(id, StandardCharsets.UTF_8.name()).replace("+", "%20")
}

/**
 * `Authorization: Bearer <management-key>` against `<server>/api`.
 */
class FuseboxApi(
    val baseUrl: String,
    val key: String,
    private val client: OkHttpClient = sharedClient,
) {
    val host: String get() = baseUrl.toHttpUrlOrNull()?.host ?: baseUrl

    fun apiUrl(path: String): String = "${baseUrl.trimEnd('/')}/api$path"

    fun liveWsUrl(): String {
        val base = apiUrl("/live")
        val wsBase = when {
            base.startsWith("https://") -> "wss://" + base.removePrefix("https://")
            base.startsWith("http://") -> "ws://" + base.removePrefix("http://")
            else -> base
        }
        val encodedKey = URLEncoder.encode(key, StandardCharsets.UTF_8.name())
        return "$wsBase?key=$encodedKey"
    }

    fun openLiveSocket(listener: WebSocketListener): WebSocket {
        val req = Request.Builder().url(liveWsUrl()).build()
        return wsClient.newWebSocket(req, listener)
    }

    suspend fun request(method: String, path: String, bodyJson: String? = null): String =
        withContext(Dispatchers.IO) {
            val builder = Request.Builder()
                .url(apiUrl(path))
                .header("Authorization", "Bearer $key")
                .header("Accept", "application/json")
            val body = bodyJson?.toRequestBody(JSON_MEDIA_TYPE)
            when (method) {
                "GET" -> builder.get()
                "DELETE" -> if (body != null) builder.delete(body) else builder.delete()
                else -> builder.method(method, body ?: "".toRequestBody(JSON_MEDIA_TYPE))
            }
            val resp = try {
                client.newCall(builder.build()).execute()
            } catch (e: IOException) {
                throw ApiException(0, describeIoError(e))
            }
            resp.use { r ->
                val text = r.body.string()
                if (r.code !in 200..299) {
                    throw ApiException(r.code, serverMessage(r.code, r.message, text))
                }
                text
            }
        }

    suspend fun <T> get(path: String, serializer: DeserializationStrategy<T>): T {
        val raw = request("GET", path)
        return FuseboxJson.decodeFromString(serializer, raw)
    }

    suspend fun <B> send(method: String, path: String, serializer: KSerializer<B>, body: B): String {
        val raw = FuseboxJson.encodeToString(serializer, body)
        return request(method, path, raw)
    }

    suspend fun overview(): Overview = get("/overview", Overview.serializer())

    suspend fun accounts(): List<Account> =
        get("/accounts", ListSerializer(Account.serializer())).map { it.normalized() }

    suspend fun faults(): List<Fault> = get("/faults", ListSerializer(Fault.serializer()))

    suspend fun dismissedFaults(): List<Fault> = get("/faults/dismissed", ListSerializer(Fault.serializer()))

    suspend fun requests(limit: Int? = null): List<RequestLog> =
        get(if (limit != null) "/requests?limit=$limit" else "/requests", ListSerializer(RequestLog.serializer()))

    suspend fun activity(id: String): AccountActivity =
        get("/accounts/${ConnectionHelper.encodePathSegment(id)}/activity", AccountActivity.serializer())

    suspend fun usageDashboard(query: String): UsageDashboard =
        get("/usage/dashboard?$query", UsageDashboard.serializer())

    suspend fun usageSpeed(query: String): UsageSpeed =
        get("/usage/speed?$query", UsageSpeed.serializer())

    suspend fun pushStatus(): PushStatus = get("/push", PushStatus.serializer())

    companion object {
        private val JSON_MEDIA_TYPE = "application/json; charset=utf-8".toMediaType()

        val sharedClient: OkHttpClient = OkHttpClient.Builder()
            .connectTimeout(15, TimeUnit.SECONDS)
            .readTimeout(20, TimeUnit.SECONDS)
            .writeTimeout(20, TimeUnit.SECONDS)
            .retryOnConnectionFailure(true)
            .build()

        val wsClient: OkHttpClient = OkHttpClient.Builder()
            .connectTimeout(15, TimeUnit.SECONDS)
            .readTimeout(0, TimeUnit.MILLISECONDS)
            .pingInterval(25, TimeUnit.SECONDS)
            .build()

        fun serverMessage(status: Int, reason: String, body: String): String {
            runCatching { FuseboxJson.decodeFromString(ErrorResponse.serializer(), body).error }
                .getOrNull()
                ?.takeIf { it.isNotBlank() }
                ?.let { return it }
            return when (status) {
                401 -> "The management key was not accepted."
                403 -> "This server only accepts management from its own machine. Set a management key in its config.yaml."
                404 -> "Not found (404). This Fusebox may be older than the app."
                else -> {
                    val trimmed = body.take(200).trim()
                    if (trimmed.isEmpty() || trimmed.startsWith("<")) "The server answered $status $reason."
                    else trimmed
                }
            }
        }

        fun describeIoError(e: IOException): String = when (e) {
            is UnknownHostException -> "Can’t find that server. On a tailnet address, check that Tailscale is connected."
            is ConnectException -> "The server refused the connection."
            is SocketTimeoutException -> "The server didn’t answer in time. On a tailnet address, check that Tailscale is connected."
            is SSLException -> "The server’s HTTPS certificate wasn’t accepted."
            else -> e.message ?: "Network error."
        }
    }
}

/**
 * Talks to the Fusebox push relay (`push.whombat.com` or self-hosted `fusebox push-relay`)
 * to register, keep alive, or remove this Android device's FCM token handle.
 */
class RelayClient(
    private val baseUrl: String,
    private val client: OkHttpClient = FuseboxApi.sharedClient,
) {
    @Serializable
    private data class RegisterBody(val platform: String = "fcm", val token: String)

    @Serializable
    private data class HandleResponse(val handle: String)

    private val root = baseUrl.trimEnd('/')

    suspend fun registerFcm(token: String): String = withContext(Dispatchers.IO) {
        val body = FuseboxJson.encodeToString(RegisterBody.serializer(), RegisterBody(token = token))
            .toRequestBody(JSON_MEDIA_TYPE)
        val req = Request.Builder().url("$root/v1/devices").post(body).build()
        val resp = try {
            client.newCall(req).execute()
        } catch (e: IOException) {
            throw ApiException(0, "The push relay is unreachable.")
        }
        resp.use { r ->
            val text = r.body.string()
            if (r.code !in 200..299) {
                val err = runCatching { FuseboxJson.decodeFromString(ErrorResponse.serializer(), text).error }.getOrNull()
                throw ApiException(r.code, err ?: "The push relay answered ${r.code}.")
            }
            FuseboxJson.decodeFromString(HandleResponse.serializer(), text).handle
        }
    }

    /** Returns true if kept, false on 410 (forgotten), null on network failure. */
    suspend fun touch(handle: String): Boolean? = withContext(Dispatchers.IO) {
        val req = Request.Builder()
            .url("$root/v1/devices/${ConnectionHelper.encodePathSegment(handle)}/touch")
            .post("".toRequestBody(null))
            .build()
        try {
            client.newCall(req).execute().use { r ->
                when (r.code) {
                    in 200..299 -> true
                    410 -> false
                    else -> null
                }
            }
        } catch (_: IOException) {
            null
        }
    }

    suspend fun unregister(handle: String) = withContext(Dispatchers.IO) {
        val req = Request.Builder()
            .url("$root/v1/devices/${ConnectionHelper.encodePathSegment(handle)}")
            .delete()
            .build()
        runCatching { client.newCall(req).execute().close() }
    }

    companion object {
        private val JSON_MEDIA_TYPE = "application/json; charset=utf-8".toMediaType()
    }
}
