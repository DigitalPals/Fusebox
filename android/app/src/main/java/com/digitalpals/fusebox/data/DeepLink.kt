package com.digitalpals.fusebox.data

import java.net.URI
import java.net.URLDecoder
import java.net.URLEncoder
import java.nio.charset.StandardCharsets

/** Where a notification tap, widget tap, or fusebox:// deep link lands. */
sealed interface DeepLink {
    data object Overview : DeepLink
    data class Account(val id: String) : DeepLink
    data object Accounts : DeepLink
    data object Requests : DeepLink
    data class Request(val id: Int) : DeepLink
    data object Usage : DeepLink
    data object Faults : DeepLink
    data object Settings : DeepLink

    companion object {
        /**
         * A notification's `path` is the dashboard's hash route (`#/accounts/<url-encoded id>`).
         * An account route opens that account; anything else opens Overview.
         */
        fun routePath(path: String?): DeepLink {
            var p = path?.trim().orEmpty()
            if (p.isEmpty()) return Overview
            if (p.startsWith("#")) p = p.drop(1)
            if (p.startsWith("/")) p = p.drop(1)
            val slash = p.indexOf('/')
            if (slash < 0) return Overview
            val head = p.substring(0, slash)
            if (head != "accounts") return Overview
            val raw = p.substring(slash + 1).substringBefore('?')
            val id = runCatching { URLDecoder.decode(raw, StandardCharsets.UTF_8.name()) }.getOrDefault(raw)
            return if (id.isEmpty()) Overview else Account(id)
        }

        /** The app's own links: `fusebox://accounts/<id>`, `fusebox://faults`, etc. */
        fun routeUri(uriString: String?): DeepLink {
            if (uriString.isNullOrBlank()) return Overview
            val uri = runCatching { URI(uriString) }.getOrNull() ?: return Overview
            if (!uri.scheme.equals("fusebox", ignoreCase = true)) return Overview
            val host = uri.host.orEmpty()
            val rest = uri.rawPath.orEmpty().trim('/')
            return when (host) {
                "accounts" -> {
                    if (rest.isEmpty()) Accounts
                    else Account(runCatching { URLDecoder.decode(rest, StandardCharsets.UTF_8.name()) }.getOrDefault(rest))
                }
                "requests" -> Requests
                "usage" -> Usage
                "faults" -> Faults
                "settings" -> Settings
                else -> Overview
            }
        }

        fun accountUri(id: String): String {
            val encoded = URLEncoder.encode(id, StandardCharsets.UTF_8.name()).replace("+", "%20")
            return "fusebox://accounts/$encoded"
        }
    }
}
