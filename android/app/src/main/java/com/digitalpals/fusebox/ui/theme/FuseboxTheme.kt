package com.digitalpals.fusebox.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.ExperimentalTextApi
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.sp
import com.digitalpals.fusebox.R
import com.digitalpals.fusebox.data.AccountState
import com.digitalpals.fusebox.data.Fault
import com.digitalpals.fusebox.data.QuotaReading

/**
 * DESIGN.md's warm near-black operator panel and daylight counterpart (matching ios/scripts/make-colors.mjs).
 * No brand amber anywhere in the app UI: accent is `fg`, colour is strictly for state and provider logos.
 */
@Immutable
data class FuseboxColors(
    val isDark: Boolean,
    val bg: Color,
    val surface: Color,
    val surfacePop: Color,
    val surfaceInset: Color,
    val segOn: Color,
    val ctlHover: Color,
    val line: Color,
    val lineSoft: Color,
    val lineStrong: Color,
    val lineHover: Color,
    val rivet: Color,
    val fg: Color,
    val fg2: Color,
    val fg3: Color,
    val idle: Color,
    val bar: Color,
    val meter: Color,
    val ok: Color,
    val warn: Color,
    val err: Color,
    val errLine: Color,
    val chart: List<Color>,
    val chartOther: Color,
    val speedLines: List<Color>,
) {
    val segOff: Color get() = line

    fun level(l: QuotaReading.Level): Color = when (l) {
        QuotaReading.Level.NORMAL -> meter
        QuotaReading.Level.WARN -> warn
        QuotaReading.Level.ERR -> err
    }

    fun state(level: String?): Color = when (level) {
        "err" -> err
        "warn" -> warn
        "ok" -> ok
        else -> fg2
    }

    fun accountState(kind: AccountState.Kind): Color = when (kind) {
        AccountState.Kind.READY -> ok
        AccountState.Kind.COOLING -> warn
        AccountState.Kind.ERROR -> err
        AccountState.Kind.DISABLED -> idle
    }

    fun faultColor(fault: Fault): Color = when (fault.level) {
        "err" -> err
        "ok" -> ok
        else -> warn
    }

    fun speedColor(slot: Int): Color = speedLines[slot % speedLines.size]
}

val DarkFuseboxColors = FuseboxColors(
    isDark = true,
    bg = Color(0xFF0B0B0A),
    surface = Color(0xFF121210),
    surfacePop = Color(0xFF141412),
    surfaceInset = Color(0xFF0E0E0C),
    segOn = Color(0xFF2A2925),
    ctlHover = Color(0xFF1C1B18),
    line = Color(0xFF24231F),
    lineSoft = Color(0xFF1D1C19),
    lineStrong = Color(0xFF33312C),
    lineHover = Color(0xFF4A4740),
    rivet = Color(0xFF2B2A26),
    fg = Color(0xFFF2EFE8),
    fg2 = Color(0xFFB4AFA4),
    fg3 = Color(0xFF857F74),
    idle = Color(0xFF55524B),
    bar = Color(0xFF3A3833),
    meter = Color(0xFFD9D4C8),
    ok = Color(0xFF4ADE80),
    warn = Color(0xFFFBBF24),
    err = Color(0xFFFB7185),
    errLine = Color(0xFF4A2A30),
    chart = listOf(
        Color(0xFFD9D4C8),
        Color(0xFFAEA99D),
        Color(0xFF857F74),
        Color(0xFF625D55),
        Color(0xFF4A4740),
    ),
    chartOther = Color(0xFF33312C),
    speedLines = listOf(
        Color(0xFFDE9589),
        Color(0xFF81BB8D),
        Color(0xFF5FBACA),
        Color(0xFF95A8E4),
        Color(0xFFCC96C6),
    ),
)

val LightFuseboxColors = FuseboxColors(
    isDark = false,
    bg = Color(0xFFF6F4EF),
    surface = Color(0xFFFCFBF8),
    surfacePop = Color(0xFFFFFFFF),
    surfaceInset = Color(0xFFF0EDE6),
    segOn = Color(0xFFE3DFD5),
    ctlHover = Color(0xFFEBE8E0),
    line = Color(0xFFE3DFD6),
    lineSoft = Color(0xFFECE9E2),
    lineStrong = Color(0xFFD2CDC2),
    lineHover = Color(0xFFA9A397),
    rivet = Color(0xFFD9D4C9),
    fg = Color(0xFF1B1A17),
    fg2 = Color(0xFF4A4741),
    fg3 = Color(0xFF6B665C),
    idle = Color(0xFFA39E92),
    bar = Color(0xFFD3CEC3),
    meter = Color(0xFF3A3833),
    ok = Color(0xFF15803D),
    warn = Color(0xFFA16207),
    err = Color(0xFFBE123C),
    errLine = Color(0xFFF0C3CC),
    chart = listOf(
        Color(0xFF2A2925),
        Color(0xFF57534B),
        Color(0xFF7F796E),
        Color(0xFFA39E92),
        Color(0xFFC2BDB1),
    ),
    chartOther = Color(0xFFDCD7CC),
    speedLines = listOf(
        Color(0xFFB06B60),
        Color(0xFF578F63),
        Color(0xFF2D8F9E),
        Color(0xFF6B7DB6),
        Color(0xFF9F6C9A),
    ),
)

val LocalFuseboxColors = staticCompositionLocalOf { DarkFuseboxColors }

@OptIn(ExperimentalTextApi::class)
object FuseboxFonts {
    val sans = FontFamily(
        Font(
            resId = R.font.ibm_plex_sans,
            weight = FontWeight.Normal,
            variationSettings = FontVariation.Settings(FontVariation.weight(400)),
        ),
        Font(
            resId = R.font.ibm_plex_sans,
            weight = FontWeight.Medium,
            variationSettings = FontVariation.Settings(FontVariation.weight(500)),
        ),
        Font(
            resId = R.font.ibm_plex_sans,
            weight = FontWeight.SemiBold,
            variationSettings = FontVariation.Settings(FontVariation.weight(600)),
        ),
    )

    val condensed = FontFamily(
        Font(resId = R.font.ibm_plex_sans_condensed_medium, weight = FontWeight.Medium),
        Font(resId = R.font.ibm_plex_sans_condensed_semibold, weight = FontWeight.SemiBold),
    )

    val mono = FontFamily(
        Font(resId = R.font.dm_mono_regular, weight = FontWeight.Normal),
        Font(resId = R.font.dm_mono_medium, weight = FontWeight.Medium),
    )
}

object Typeface {
    private const val TABULAR = "tnum"

    fun sans(size: TextUnit, weight: FontWeight = FontWeight.Normal, lineHeight: TextUnit = TextUnit.Unspecified): TextStyle =
        TextStyle(
            fontFamily = FuseboxFonts.sans,
            fontWeight = weight,
            fontSize = size,
            lineHeight = lineHeight,
            fontFeatureSettings = TABULAR,
        )

    fun label(size: TextUnit = 11.sp): TextStyle = TextStyle(
        fontFamily = FuseboxFonts.condensed,
        fontWeight = FontWeight.SemiBold,
        fontSize = size,
        letterSpacing = (size.value * 0.12f).sp,
        fontFeatureSettings = TABULAR,
    )

    fun data(size: TextUnit = 12.sp, weight: FontWeight = FontWeight.Normal): TextStyle = TextStyle(
        fontFamily = FuseboxFonts.mono,
        fontWeight = weight,
        fontSize = size,
        fontFeatureSettings = TABULAR,
    )

    val body = sans(15.sp, FontWeight.Normal, 21.sp)
    val row = sans(14.sp, FontWeight.Normal, 19.sp)
    val rowMedium = sans(14.sp, FontWeight.Medium, 19.sp)
    val meta = sans(13.sp, FontWeight.Normal, 18.sp)
    val small = sans(12.sp, FontWeight.Normal, 16.sp)
    val figure = sans(20.sp, FontWeight.Medium, 23.sp).copy(letterSpacing = (-0.2).sp)
    val bigFigure = sans(30.sp, FontWeight.Medium, 34.sp).copy(letterSpacing = (-0.6).sp)
    val title = sans(20.sp, FontWeight.SemiBold, 25.sp)
    val pageTitle = TextStyle(
        fontFamily = FuseboxFonts.condensed,
        fontWeight = FontWeight.SemiBold,
        fontSize = 22.sp,
        letterSpacing = 1.8.sp,
        fontFeatureSettings = TABULAR,
    )
}

@Composable
fun FuseboxTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    content: @Composable () -> Unit,
) {
    val colors = if (darkTheme) DarkFuseboxColors else LightFuseboxColors
    val m3 = if (darkTheme) {
        darkColorScheme(
            primary = colors.fg,
            onPrimary = colors.bg,
            secondary = colors.fg2,
            onSecondary = colors.bg,
            background = colors.bg,
            onBackground = colors.fg,
            surface = colors.surface,
            onSurface = colors.fg,
            surfaceVariant = colors.surfaceInset,
            onSurfaceVariant = colors.fg2,
            outline = colors.lineStrong,
            outlineVariant = colors.line,
            error = colors.err,
            onError = colors.bg,
        )
    } else {
        lightColorScheme(
            primary = colors.fg,
            onPrimary = colors.bg,
            secondary = colors.fg2,
            onSecondary = colors.bg,
            background = colors.bg,
            onBackground = colors.fg,
            surface = colors.surface,
            onSurface = colors.fg,
            surfaceVariant = colors.surfaceInset,
            onSurfaceVariant = colors.fg2,
            outline = colors.lineStrong,
            outlineVariant = colors.line,
            error = colors.err,
            onError = colors.bg,
        )
    }
    CompositionLocalProvider(LocalFuseboxColors provides colors) {
        MaterialTheme(colorScheme = m3, content = content)
    }
}
