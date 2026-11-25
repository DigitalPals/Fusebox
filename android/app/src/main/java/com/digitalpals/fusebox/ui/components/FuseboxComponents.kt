package com.digitalpals.fusebox.ui.components

import androidx.annotation.DrawableRes
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowForward
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.digitalpals.fusebox.R
import com.digitalpals.fusebox.data.Account
import com.digitalpals.fusebox.data.AccountState
import com.digitalpals.fusebox.data.Fmt
import com.digitalpals.fusebox.data.LiveState
import com.digitalpals.fusebox.data.MeterModel
import com.digitalpals.fusebox.data.MinuteBucket
import com.digitalpals.fusebox.data.QuotaDisplay
import com.digitalpals.fusebox.data.QuotaReading
import com.digitalpals.fusebox.data.QuotaWindow
import com.digitalpals.fusebox.data.Quotas
import com.digitalpals.fusebox.data.noShortLimit
import com.digitalpals.fusebox.data.scopedWindows
import com.digitalpals.fusebox.data.state
import com.digitalpals.fusebox.data.statusWord
import com.digitalpals.fusebox.data.window
import com.digitalpals.fusebox.ui.theme.LocalFuseboxColors
import com.digitalpals.fusebox.ui.theme.Typeface
import kotlinx.coroutines.delay
import java.time.Instant
import kotlin.math.max
import kotlin.math.round

/** Emits a ticking Instant every 30 seconds so countdowns and "ago" labels stay live. */
@Composable
fun rememberTicker(periodMs: Long = 30_000L): Instant {
    var now by remember { mutableStateOf(Instant.now()) }
    LaunchedEffect(periodMs) {
        while (true) {
            delay(periodMs)
            now = Instant.now()
        }
    }
    return now
}

// MARK: - Panels, Hairlines, Heads

@Composable
fun CardPanel(
    modifier: Modifier = Modifier,
    rivets: Boolean = false,
    padding: Dp = 16.dp,
    spacing: Dp = 12.dp,
    borderColor: Color? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    val colors = LocalFuseboxColors.current
    val shape = RoundedCornerShape(8.dp)
    Box(
        modifier = modifier
            .fillMaxWidth()
            .clip(shape)
            .background(colors.surface, shape)
            .border(1.dp, borderColor ?: colors.line, shape),
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(padding),
            verticalArrangement = Arrangement.spacedBy(spacing),
            content = content,
        )
        if (rivets) {
            Canvas(modifier = Modifier.matchParentSize()) {
                val r = 2.5.dp.toPx()
                val inset = 7.dp.toPx()
                val c = colors.rivet
                drawCircle(c, r, Offset(inset, inset))
                drawCircle(c, r, Offset(size.width - inset, inset))
                drawCircle(c, r, Offset(inset, size.height - inset))
                drawCircle(c, r, Offset(size.width - inset, size.height - inset))
            }
        }
    }
}

@Composable
fun Hairline(modifier: Modifier = Modifier, color: Color? = null) {
    val colors = LocalFuseboxColors.current
    Box(
        modifier = modifier
            .fillMaxWidth()
            .height(1.dp)
            .background(color ?: colors.line),
    )
}

@Composable
fun CapsLabel(
    text: String,
    modifier: Modifier = Modifier,
    size: TextUnit = 11.sp,
    color: Color? = null,
    isHeader: Boolean = false,
) {
    val colors = LocalFuseboxColors.current
    Text(
        text = text.uppercase(),
        style = Typeface.label(size),
        color = color ?: colors.fg3,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
        modifier = if (isHeader) modifier.semantics { heading() } else modifier,
    )
}

@Composable
fun PanelHead(
    title: String,
    modifier: Modifier = Modifier,
    trailing: (@Composable RowScope.() -> Unit)? = null,
) {
    Row(
        modifier = modifier
            .fillMaxWidth()
            .defaultMinSize(minHeight = 28.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        CapsLabel(
            text = title,
            size = 11.5.sp,
            isHeader = true,
            modifier = Modifier.weight(1f),
        )
        if (trailing != null) trailing()
    }
}

@Composable
fun SectionHead(
    title: String,
    meta: String? = null,
    modifier: Modifier = Modifier,
    trailing: (@Composable RowScope.() -> Unit)? = null,
) {
    val colors = LocalFuseboxColors.current
    Column(modifier = modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            CapsLabel(
                text = title,
                size = 12.5.sp,
                color = colors.fg,
                isHeader = true,
                modifier = Modifier.weight(1f),
            )
            if (trailing != null) trailing()
        }
        if (!meta.isNullOrEmpty()) {
            Text(text = meta, style = Typeface.meta, color = colors.fg3)
        }
    }
}

@Composable
fun KeyValueRow(
    key: String,
    value: String,
    mono: Boolean = false,
    valueColor: Color? = null,
    modifier: Modifier = Modifier,
) {
    val colors = LocalFuseboxColors.current
    val vStyle = if (mono) Typeface.data(13.sp) else Typeface.row
    val vColor = valueColor ?: colors.fg
    if (value.length > 30) {
        Column(
            modifier = modifier
                .fillMaxWidth()
                .padding(vertical = 10.dp),
            verticalArrangement = Arrangement.spacedBy(3.dp),
        ) {
            Text(key, style = Typeface.row, color = colors.fg3)
            Text(value, style = vStyle, color = vColor)
        }
    } else {
        Row(
            modifier = modifier
                .fillMaxWidth()
                .padding(vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(key, style = Typeface.row, color = colors.fg3)
            Spacer(Modifier.weight(1f))
            Text(
                text = value,
                style = vStyle,
                color = vColor,
                textAlign = TextAlign.End,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

@Composable
fun LinkButton(
    title: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = LocalFuseboxColors.current
    Row(
        modifier = modifier
            .defaultMinSize(minHeight = 40.dp)
            .clip(RoundedCornerShape(6.dp))
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Text(title, style = Typeface.meta, color = colors.fg2)
        Icon(
            imageVector = Icons.AutoMirrored.Filled.ArrowForward,
            contentDescription = null,
            tint = colors.fg2,
            modifier = Modifier.size(13.dp),
        )
    }
}

@Composable
fun EmptyState(
    title: String,
    icon: ImageVector,
    message: String? = null,
    modifier: Modifier = Modifier,
) {
    val colors = LocalFuseboxColors.current
    Column(
        modifier = modifier
            .fillMaxWidth()
            .padding(32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(imageVector = icon, contentDescription = null, tint = colors.fg3, modifier = Modifier.size(32.dp))
        Text(title, style = Typeface.sans(17.sp, FontWeight.SemiBold), color = colors.fg, textAlign = TextAlign.Center)
        if (!message.isNullOrEmpty()) {
            Text(message, style = Typeface.row, color = colors.fg3, textAlign = TextAlign.Center)
        }
    }
}

// MARK: - Buttons, Switch, Segmented Control, Input

enum class FuseButtonKind { NORMAL, PRIMARY, DESTRUCTIVE, QUIET }

@Composable
fun FuseButton(
    text: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    kind: FuseButtonKind = FuseButtonKind.NORMAL,
    compact: Boolean = false,
    fill: Boolean = false,
    enabled: Boolean = true,
) {
    val colors = LocalFuseboxColors.current
    val shape = RoundedCornerShape(if (compact) 6.dp else 8.dp)
    val minHeight = if (compact) 34.dp else 44.dp
    val textColor = when {
        !enabled -> colors.idle
        kind == FuseButtonKind.PRIMARY -> colors.bg
        kind == FuseButtonKind.DESTRUCTIVE -> colors.err
        kind == FuseButtonKind.QUIET -> colors.fg2
        else -> colors.fg
    }
    val bgColor = when {
        kind != FuseButtonKind.PRIMARY -> Color.Transparent
        enabled -> colors.fg
        else -> colors.ctlHover
    }
    val borderColor = when {
        kind == FuseButtonKind.QUIET -> Color.Transparent
        !enabled -> if (kind == FuseButtonKind.PRIMARY) colors.ctlHover else colors.line
        kind == FuseButtonKind.PRIMARY -> colors.fg
        else -> colors.lineStrong
    }
    Box(
        modifier = modifier
            .then(if (fill) Modifier.fillMaxWidth() else Modifier)
            .defaultMinSize(minHeight = minHeight)
            .clip(shape)
            .background(bgColor, shape)
            .border(1.dp, borderColor, shape)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(horizontal = if (compact) 12.dp else 16.dp, vertical = if (compact) 6.dp else 10.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = text,
            style = if (compact) Typeface.meta else Typeface.sans(15.sp, if (kind == FuseButtonKind.PRIMARY) FontWeight.Medium else FontWeight.Normal),
            color = textColor,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** The dashboard's breaker switch: 42×24dp, 5dp corners, 16dp square knob, `meter` track when on. */
@Composable
fun BreakerSwitch(
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    contentDesc: String = "Toggle",
) {
    val colors = LocalFuseboxColors.current
    val trackColor by animateColorAsState(if (checked) colors.meter else colors.ctlHover, tween(120), label = "track")
    val borderColor by animateColorAsState(if (checked) colors.meter else colors.lineStrong, tween(120), label = "border")
    val knobColor by animateColorAsState(if (checked) colors.bg else colors.idle, tween(120), label = "knob")
    val knobOffset by animateDpAsState(if (checked) 22.dp else 4.dp, tween(120), label = "offset")

    Box(
        modifier = modifier
            .defaultMinSize(minWidth = 44.dp, minHeight = 44.dp)
            .alpha(if (enabled) 1f else 0.45f)
            .clickable(
                enabled = enabled,
                role = Role.Switch,
                interactionSource = remember { MutableInteractionSource() },
                indication = null,
            ) { onCheckedChange(!checked) }
            .semantics {
                contentDescription = contentDesc
                stateDescription = if (checked) "On" else "Off"
            },
        contentAlignment = Alignment.Center,
    ) {
        Box(
            modifier = Modifier
                .size(width = 42.dp, height = 24.dp)
                .clip(RoundedCornerShape(5.dp))
                .background(trackColor)
                .border(1.dp, borderColor, RoundedCornerShape(5.dp)),
        ) {
            Box(
                modifier = Modifier
                    .align(Alignment.CenterStart)
                    .offset(x = knobOffset)
                    .size(16.dp)
                    .clip(RoundedCornerShape(3.dp))
                    .background(knobColor),
            )
        }
    }
}

/** The dashboard's segmented control: 2dp padding inside a 1dp `lineStrong` border; active pill `segOn`. */
@Composable
fun <T> FuseSegmented(
    options: List<Pair<T, String>>,
    selected: T,
    onSelect: (T) -> Unit,
    modifier: Modifier = Modifier,
    fill: Boolean = false,
    enabled: Boolean = true,
) {
    val colors = LocalFuseboxColors.current
    val outerShape = RoundedCornerShape(8.dp)
    val pillShape = RoundedCornerShape(6.dp)
    Row(
        modifier = modifier
            .then(if (fill) Modifier.fillMaxWidth() else Modifier.width(IntrinsicSize.Max))
            .alpha(if (enabled) 1f else 0.5f)
            .border(1.dp, colors.lineStrong, outerShape)
            .padding(2.dp),
        horizontalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        options.forEach { (value, title) ->
            val on = value == selected
            Box(
                modifier = Modifier
                    .then(if (fill) Modifier.weight(1f) else Modifier)
                    .defaultMinSize(minHeight = 32.dp)
                    .clip(pillShape)
                    .background(if (on) colors.segOn else Color.Transparent, pillShape)
                    .clickable(enabled = enabled, role = Role.Tab) { onSelect(value) }
                    .semantics { this.selected = on }
                    .padding(horizontal = 10.dp, vertical = 6.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    text = title,
                    style = Typeface.meta,
                    color = if (on) colors.fg else colors.fg3,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

@Composable
fun FuseTextField(
    value: String,
    onValueChange: (String) -> Unit,
    placeholder: String,
    modifier: Modifier = Modifier,
    visualTransformation: VisualTransformation = VisualTransformation.None,
    keyboardOptions: KeyboardOptions = KeyboardOptions.Default,
    keyboardActions: KeyboardActions = KeyboardActions.Default,
    singleLine: Boolean = true,
) {
    val colors = LocalFuseboxColors.current
    var focused by remember { mutableStateOf(false) }
    val shape = RoundedCornerShape(8.dp)
    BasicTextField(
        value = value,
        onValueChange = onValueChange,
        singleLine = singleLine,
        textStyle = Typeface.data(15.sp).copy(color = colors.fg),
        cursorBrush = SolidColor(colors.fg),
        visualTransformation = visualTransformation,
        keyboardOptions = keyboardOptions,
        keyboardActions = keyboardActions,
        modifier = modifier
            .fillMaxWidth()
            .defaultMinSize(minHeight = 46.dp)
            .onFocusChanged { focused = it.isFocused }
            .clip(shape)
            .background(colors.bg, shape)
            .border(1.dp, if (focused) colors.lineHover else colors.lineStrong, shape)
            .padding(horizontal = 12.dp, vertical = 12.dp),
        decorationBox = { inner ->
            Box(contentAlignment = Alignment.CenterStart) {
                if (value.isEmpty()) {
                    Text(placeholder, style = Typeface.data(15.sp), color = colors.fg3, maxLines = 1)
                }
                inner()
            }
        },
    )
}

// MARK: - Provider logos, dots, status, figures

private val monochromeLogos = setOf(
    "logo-codex", "logo-xai", "logo-xai-api", "logo-openrouter",
    "logo-ollama", "logo-lmstudio", "logo-groq", "logo-compat",
)

@DrawableRes
fun logoDrawableRes(name: String): Int = when (name) {
    "logo-claude" -> R.drawable.logo_claude
    "logo-codex" -> R.drawable.logo_codex
    "logo-gemini" -> R.drawable.logo_gemini
    "logo-vertex" -> R.drawable.logo_vertex
    "logo-antigravity" -> R.drawable.logo_antigravity
    "logo-xai" -> R.drawable.logo_xai
    "logo-xai-api" -> R.drawable.logo_xai_api
    "logo-kimi" -> R.drawable.logo_kimi
    "logo-meta" -> R.drawable.logo_meta
    "logo-devin" -> R.drawable.logo_devin
    "logo-openrouter" -> R.drawable.logo_openrouter
    "logo-ollama" -> R.drawable.logo_ollama
    "logo-lmstudio" -> R.drawable.logo_lmstudio
    "logo-deepseek" -> R.drawable.logo_deepseek
    "logo-groq" -> R.drawable.logo_groq
    "logo-mistral" -> R.drawable.logo_mistral
    "logo-qwen" -> R.drawable.logo_qwen
    else -> R.drawable.logo_compat
}

@Composable
fun ProviderLogo(name: String, size: Dp = 18.dp, modifier: Modifier = Modifier) {
    val colors = LocalFuseboxColors.current
    val tint = if (name in monochromeLogos) ColorFilter.tint(colors.fg) else null
    Image(
        painter = painterResource(logoDrawableRes(name)),
        contentDescription = null,
        colorFilter = tint,
        modifier = modifier.size(size),
    )
}

@Composable
fun StatusDot(color: Color, size: Dp = 8.dp, modifier: Modifier = Modifier) {
    Box(modifier = modifier.size(size).clip(CircleShape).background(color))
}

@Composable
fun LiveBadge(state: LiveState, modifier: Modifier = Modifier) {
    val colors = LocalFuseboxColors.current
    val dot = when (state) {
        LiveState.LIVE -> colors.ok
        LiveState.CONNECTING -> colors.idle
        LiveState.OFFLINE -> colors.warn
    }
    val text = when (state) {
        LiveState.LIVE -> "Live"
        LiveState.CONNECTING -> "Connecting"
        LiveState.OFFLINE -> "Reconnecting"
    }
    Row(
        modifier = modifier,
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(7.dp),
    ) {
        StatusDot(dot)
        Text(text, style = Typeface.meta, color = colors.fg2)
    }
}

@Composable
fun StatusLabel(
    account: Account,
    inFlight: Int = 0,
    now: Instant = rememberTicker(),
    modifier: Modifier = Modifier,
) {
    val colors = LocalFuseboxColors.current
    val st = account.state(now)
    val textColor = when (st.kind) {
        AccountState.Kind.READY -> colors.fg2
        AccountState.Kind.COOLING -> colors.warn
        AccountState.Kind.ERROR -> colors.err
        AccountState.Kind.DISABLED -> colors.fg3
    }
    Row(
        modifier = modifier,
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        StatusDot(colors.accountState(st.kind), size = 7.dp)
        Text(
            text = account.statusWord(inFlight, now),
            style = Typeface.meta,
            color = textColor,
            maxLines = 1,
        )
    }
}

@Composable
fun Figure(
    value: String,
    label: String,
    color: Color? = null,
    modifier: Modifier = Modifier,
) {
    val colors = LocalFuseboxColors.current
    Column(modifier = modifier, verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(
            text = value,
            style = Typeface.figure,
            color = color ?: colors.fg,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Text(
            text = label,
            style = Typeface.small,
            color = colors.fg3,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

// MARK: - Segmented Quota Meter & Rows

/**
 * 20 square segments of 5% with 2dp gaps and 1dp corners, lit for the shown share and coloured
 * by how much is used. Even-pace mark drawn as a 2dp `fg3` tick overhanging above & below with
 * a 1dp `knockout` gap on each side so it stays readable over lit segments in light and dark.
 */
@Composable
fun SegmentedMeter(
    reading: QuotaReading?,
    pace: Double? = null,
    mode: QuotaDisplay = QuotaDisplay.standard,
    barHeight: Dp = 9.dp,
    label: String = "Quota",
    knockout: Color? = null,
    modifier: Modifier = Modifier,
) {
    val colors = LocalFuseboxColors.current
    val model = MeterModel.build(reading, pace, mode)
    val over = if (barHeight >= 14.dp) 4.dp else 3.dp
    val litColor = model.level?.let { colors.level(it) } ?: colors.segOff
    val offColor = colors.segOff
    val knockoutColor = knockout ?: colors.surface
    val tickColor = colors.fg3
    val desc = if (reading == null) {
        "$label: not reported"
    } else {
        "$label: ${Fmt.percent(reading.value(mode))} ${mode.word}, ${reading.status}"
    }

    Canvas(
        modifier = modifier
            .fillMaxWidth()
            .height(barHeight + over * 2)
            .semantics { contentDescription = desc },
    ) {
        val gapPx = 2.dp.toPx()
        val overPx = over.toPx()
        val barH = barHeight.toPx()
        val radius = CornerRadius(1.dp.toPx(), 1.dp.toPx())
        val n = MeterModel.SEGMENTS
        val segW = (size.width - gapPx * (n - 1)) / n
        if (segW <= 0.5f) return@Canvas

        for (i in 0 until n) {
            val x0 = round(i * (segW + gapPx))
            val x1 = round(i * (segW + gapPx) + segW)
            drawRoundRect(
                color = if (model.isLit(i)) litColor else offColor,
                topLeft = Offset(x0, overPx),
                size = Size(max(1f, x1 - x0), barH),
                cornerRadius = radius,
            )
        }
        model.pace?.let { p ->
            val tickW = 2.dp.toPx()
            val koPad = 1.dp.toPx()
            val cx = (size.width * p.toFloat()).coerceIn(1.dp.toPx(), size.width - 1.dp.toPx())
            val tx = round(cx - tickW / 2f)
            drawRect(
                color = knockoutColor,
                topLeft = Offset(tx - koPad, overPx),
                size = Size(tickW + koPad * 2f, barH),
            )
            drawRoundRect(
                color = tickColor,
                topLeft = Offset(tx, 0f),
                size = Size(tickW, size.height),
                cornerRadius = radius,
            )
        }
    }
}

@Composable
fun MeterRow(
    label: String,
    reading: QuotaReading?,
    pace: Double?,
    mode: QuotaDisplay,
    resetsAt: Instant? = null,
    window: QuotaWindow? = null,
    runsOutEarly: Boolean = false,
    labelWidth: Dp = 28.dp,
    child: Boolean = false,
    now: Instant = rememberTicker(),
) {
    val colors = LocalFuseboxColors.current
    val childTint = if (child) MeterModel.childLevel(reading)?.let { colors.level(it) } else null
    val pctColor = when {
        reading == null -> colors.fg3
        childTint != null -> childTint
        reading.level != QuotaReading.Level.NORMAL -> colors.level(reading.level)
        else -> colors.fg
    }
    val outAt = window?.let { Quotas.runsOut(it, now) }
    val targetReset = window?.resetsAt ?: resetsAt
    val resetText: String
    val resetColor: Color
    when {
        outAt != null -> {
            resetText = "out ${Fmt.compactLeft(outAt, now)}"
            resetColor = colors.warn
        }
        targetReset != null && targetReset.isAfter(now) -> {
            resetText = "↺ ${Fmt.compactLeft(targetReset, now)}"
            resetColor = if (runsOutEarly) colors.warn else childTint ?: colors.fg3
        }
        else -> {
            resetText = ""
            resetColor = colors.fg3
        }
    }

    Row(
        modifier = Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        if (child) {
            Text(
                text = "↳ $label",
                style = Typeface.small,
                color = childTint ?: colors.fg3,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.width(labelWidth),
            )
        } else {
            CapsLabel(
                text = label,
                size = 10.5.sp,
                color = colors.fg3,
                modifier = Modifier.width(labelWidth),
            )
        }
        SegmentedMeter(
            reading = reading,
            pace = pace,
            mode = mode,
            barHeight = 9.dp,
            label = label,
            modifier = Modifier.weight(1f),
        )
        Text(
            text = reading?.let { Fmt.percent(it.value(mode)) } ?: "–",
            style = Typeface.data(12.sp),
            color = pctColor,
            textAlign = TextAlign.End,
            maxLines = 1,
            modifier = Modifier.width(36.dp),
        )
        Text(
            text = resetText,
            style = Typeface.data(11.5.sp),
            color = resetColor,
            textAlign = TextAlign.End,
            maxLines = 1,
            modifier = Modifier.width(64.dp),
        )
    }
}

@Composable
fun AccountMeters(
    account: Account,
    mode: QuotaDisplay,
    scoped: Boolean = false,
    labelWidth: Dp = 28.dp,
    now: Instant = rememberTicker(),
) {
    val colors = LocalFuseboxColors.current
    val w5 = account.window(short = true, now = now)
    val ww = account.window(short = false, now = now)
    val scopedList = if (scoped) account.scopedWindows(now) else emptyList()
    val grouped = scopedList.groupBy { it.model ?: "" }

    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        if (!account.noShortLimit(now)) {
            MeterRow(
                label = Quotas.label(w5, short = true),
                reading = QuotaReading.of(w5, now),
                pace = Quotas.elapsed(w5, now),
                mode = mode,
                window = w5,
                labelWidth = labelWidth,
                now = now,
            )
        }
        MeterRow(
            label = Quotas.label(ww, short = false),
            reading = QuotaReading.of(ww, now),
            pace = Quotas.elapsed(ww, now),
            mode = mode,
            window = ww,
            labelWidth = labelWidth,
            now = now,
        )
        grouped.forEach { (model, wins) ->
            if (wins.size == 1 && !Quotas.isShort(wins[0])) {
                val w = wins[0]
                MeterRow(
                    label = Fmt.capitalized(model),
                    reading = QuotaReading.of(w, now),
                    pace = Quotas.elapsed(w, now),
                    mode = mode,
                    window = w,
                    labelWidth = labelWidth,
                    child = true,
                    now = now,
                )
            } else {
                Text(
                    text = "↳ ${Fmt.capitalized(model)}",
                    style = Typeface.small,
                    color = colors.fg3,
                    modifier = Modifier.padding(top = 4.dp),
                )
                wins.forEach { w ->
                    MeterRow(
                        label = Quotas.label(w, short = Quotas.isShort(w)),
                        reading = QuotaReading.of(w, now),
                        pace = Quotas.elapsed(w, now),
                        mode = mode,
                        window = w,
                        labelWidth = labelWidth,
                        now = now,
                    )
                }
            }
        }
    }
}

// MARK: - Load Chart (Canvas: 60 one-minute columns, current minute in `meter`, failed in `err`)

@Composable
fun LoadChart(
    series: List<MinuteBucket>,
    height: Dp = 64.dp,
    modifier: Modifier = Modifier,
) {
    val colors = LocalFuseboxColors.current
    val nowMin = Instant.now().epochSecond / 60
    val maxVal = max(4, series.maxOfOrNull { it.requests } ?: 0)
    val totalReqs = series.sumOf { it.requests }

    Canvas(
        modifier = modifier
            .fillMaxWidth()
            .height(height)
            .semantics { contentDescription = "${Fmt.plural(totalReqs, "request")} in the last hour" },
    ) {
        val count = series.size.coerceAtLeast(1)
        val slot = size.width / count
        val barW = (slot * 0.72f).coerceAtLeast(1.5f)
        val pad = (slot - barW) / 2f
        val chartH = size.height - 1.dp.toPx()

        series.forEachIndexed { idx, b ->
            if (b.requests <= 0) return@forEachIndexed
            val x = idx * slot + pad
            val failed = (b.failed ?: 0).coerceIn(0, b.requests)
            val ok = b.requests - failed
            val okH = (ok.toFloat() / maxVal) * chartH
            val failH = (failed.toFloat() / maxVal) * chartH
            if (ok > 0) {
                drawRect(
                    color = if (b.minute == nowMin) colors.meter else colors.bar,
                    topLeft = Offset(x, chartH - okH),
                    size = Size(barW, okH),
                )
            }
            if (failed > 0) {
                drawRect(
                    color = colors.err,
                    topLeft = Offset(x, chartH - okH - failH),
                    size = Size(barW, failH),
                )
            }
        }
        drawLine(
            color = colors.line,
            start = Offset(0f, size.height - 0.5.dp.toPx()),
            end = Offset(size.width, size.height - 0.5.dp.toPx()),
            strokeWidth = 1.dp.toPx(),
        )
    }
}

// MARK: - Toast banner

@Composable
fun ToastBanner(text: String?, onDismiss: () -> Unit, modifier: Modifier = Modifier) {
    val colors = LocalFuseboxColors.current
    LaunchedEffect(text) {
        if (text != null) {
            delay(2400L)
            onDismiss()
        }
    }
    AnimatedVisibility(
        visible = text != null,
        enter = fadeIn(tween(150)),
        exit = fadeOut(tween(180)),
        modifier = modifier,
    ) {
        Box(
            modifier = Modifier
                .padding(bottom = 20.dp)
                .clip(RoundedCornerShape(8.dp))
                .background(colors.fg)
                .padding(horizontal = 14.dp, vertical = 10.dp),
        ) {
            Text(text.orEmpty(), style = Typeface.meta, color = colors.bg)
        }
    }
}
