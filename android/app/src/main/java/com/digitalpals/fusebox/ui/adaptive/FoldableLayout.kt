package com.digitalpals.fusebox.ui.adaptive

import android.app.Activity
import android.graphics.Rect
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.window.layout.FoldingFeature
import androidx.window.layout.WindowInfoTracker
import com.digitalpals.fusebox.ui.theme.LocalFuseboxColors
import kotlin.math.ceil
import kotlin.math.max
import kotlin.math.min

/**
 * Current window size class and foldable hinge posture from Jetpack WindowManager.
 * Drives Option A adaptive layouts:
 *  - Compact (< 600dp, folded cover display / standard phone): bottom nav, 1 column, stacked detail.
 *  - Medium/Expanded (>= 600dp, unfolded Pixel Fold / Z Fold / tablet): navigation rail,
 *    2-pane List-Detail for Accounts & Requests (hinge-aware), 2-column Overview & Usage grids.
 *  - Half-opened book or tabletop posture (including Flip): splits cleanly around the hinge bounds.
 */
@Immutable
data class FoldableState(
    val widthDp: Dp = 400.dp,
    val heightDp: Dp = 800.dp,
    val hingeBoundsPx: Rect? = null,
    val hingeBoundsDp: HingeBoundsDp? = null,
    val isHingeVertical: Boolean = false,
    val isHingeHorizontal: Boolean = false,
    val isSeparating: Boolean = false,
    val isHalfOpened: Boolean = false,
) {
    /** Medium or Expanded width (>= 600dp): unfolded book-style foldable or tablet. */
    val useNavigationRail: Boolean get() = widthDp >= 600.dp

    /** True when two panes (List-Detail or two card columns) fit comfortably. */
    val supportsTwoPanes: Boolean get() = widthDp >= 600.dp || (isHingeVertical && isSeparating && widthDp >= 520.dp)

    val isBookPosture: Boolean get() = isHingeVertical && isHalfOpened
    val isTabletopPosture: Boolean get() = isHingeHorizontal && isHalfOpened
}

@Immutable
data class HingeBoundsDp(
    val left: Dp,
    val top: Dp,
    val right: Dp,
    val bottom: Dp,
) {
    val width: Dp get() = (right - left).coerceAtLeast(0.dp)
    val height: Dp get() = (bottom - top).coerceAtLeast(0.dp)
}

val LocalFoldableState = staticCompositionLocalOf { FoldableState() }

@Composable
fun ProvideFoldableState(content: @Composable () -> Unit) {
    val context = LocalContext.current
    val activity = context as? Activity
    val config = LocalConfiguration.current
    val density = LocalDensity.current

    var foldingFeature by remember { mutableStateOf<FoldingFeature?>(null) }

    LaunchedEffect(activity) {
        if (activity == null) return@LaunchedEffect
        WindowInfoTracker.getOrCreate(activity)
            .windowLayoutInfo(activity)
            .collect { info ->
                foldingFeature = info.displayFeatures.filterIsInstance<FoldingFeature>().firstOrNull()
            }
    }

    val widthDp = config.screenWidthDp.dp
    val heightDp = config.screenHeightDp.dp
    val ff = foldingFeature
    val hingeDp = ff?.bounds?.let { b ->
        with(density) {
            HingeBoundsDp(
                left = b.left.toDp(),
                top = b.top.toDp(),
                right = b.right.toDp(),
                bottom = b.bottom.toDp(),
            )
        }
    }

    val state = FoldableState(
        widthDp = widthDp,
        heightDp = heightDp,
        hingeBoundsPx = ff?.bounds,
        hingeBoundsDp = hingeDp,
        isHingeVertical = ff?.orientation == FoldingFeature.Orientation.VERTICAL,
        isHingeHorizontal = ff?.orientation == FoldingFeature.Orientation.HORIZONTAL,
        isSeparating = ff?.isSeparating == true,
        isHalfOpened = ff?.state == FoldingFeature.State.HALF_OPENED,
    )

    CompositionLocalProvider(LocalFoldableState provides state, content = content)
}

/** Pure helper tested by unit tests: chooses 1..3 columns and whether to split along a vertical hinge. */
data class ColumnPlan(
    val columns: Int,
    val leftFraction: Float?,
    val hingeGapDp: Float,
) {
    companion object {
        fun compute(
            availableWidthDp: Float,
            minColumnWidthDp: Float,
            spacingDp: Float,
            verticalHingeLeftDp: Float? = null,
            verticalHingeRightDp: Float? = null,
            hingeSeparating: Boolean = false,
        ): ColumnPlan {
            val w = max(availableWidthDp, 1f)
            val naturalCols = max(1, min(2, ((w + spacingDp) / (minColumnWidthDp + spacingDp)).toInt()))
            if (verticalHingeLeftDp != null && verticalHingeRightDp != null && verticalHingeRightDp > verticalHingeLeftDp) {
                val left = verticalHingeLeftDp
                val right = w - verticalHingeRightDp
                val gap = max(spacingDp, verticalHingeRightDp - verticalHingeLeftDp)
                if ((hingeSeparating || naturalCols == 2) && left >= 240f && right >= 240f && gap < w / 3f) {
                    val contentWidth = left + right
                    return ColumnPlan(
                        columns = 2,
                        leftFraction = (left / contentWidth).coerceIn(0.35f, 0.65f),
                        hingeGapDp = gap,
                    )
                }
            }
            return ColumnPlan(columns = naturalCols, leftFraction = null, hingeGapDp = spacingDp)
        }

        fun <T> distribute(items: List<T>, columns: Int): List<List<T>> {
            if (columns <= 1 || items.size <= 1) return listOf(items)
            val per = ceil(items.size.toDouble() / columns.toDouble()).toInt().coerceAtLeast(1)
            return items.chunked(per)
        }
    }
}

/**
 * Lays out cards or row groups into 1 column on compact screens and 2 columns on unfolded foldables,
 * placing columns on either side of a vertical hinge so no card straddles the fold.
 */
@Composable
fun <T> FoldAwareColumns(
    items: List<T>,
    minColumnWidth: Dp = 320.dp,
    spacing: Dp = 16.dp,
    modifier: Modifier = Modifier,
    columnContent: @Composable (List<T>) -> Unit,
) {
    if (items.isEmpty()) return
    val foldable = LocalFoldableState.current
    BoxWithConstraints(modifier = modifier.fillMaxWidth()) {
        val avail = maxWidth.value
        val railOffset = if (foldable.useNavigationRail) 80f else 0f
        val hingeLeft = if (foldable.isHingeVertical) foldable.hingeBoundsDp?.left?.value?.minus(railOffset) else null
        val hingeRight = if (foldable.isHingeVertical) foldable.hingeBoundsDp?.right?.value?.minus(railOffset) else null

        val plan = ColumnPlan.compute(
            availableWidthDp = avail,
            minColumnWidthDp = minColumnWidth.value,
            spacingDp = spacing.value,
            verticalHingeLeftDp = hingeLeft?.takeIf { it > 0f },
            verticalHingeRightDp = hingeRight?.takeIf { it < avail },
            hingeSeparating = foldable.isSeparating,
        )

        val buckets = ColumnPlan.distribute(items, plan.columns)
        if (buckets.size == 1) {
            columnContent(buckets[0])
        } else {
            val leftWeight = plan.leftFraction ?: 0.5f
            val rightWeight = (1f - leftWeight).coerceAtLeast(0.25f)
            Row(modifier = Modifier.fillMaxWidth()) {
                Column(modifier = Modifier.weight(leftWeight)) {
                    columnContent(buckets[0])
                }
                Spacer(Modifier.width(plan.hingeGapDp.dp))
                Column(modifier = Modifier.weight(rightWeight)) {
                    if (buckets.size > 1) columnContent(buckets[1])
                }
            }
        }
    }
}

/**
 * Adaptive List-Detail container for Accounts and Requests:
 * - On unfolded foldables (>= 600dp or vertical separating fold): renders list on the left pane
 *   and detail on the right pane, aligned around the hinge with a hairline divider.
 * - On compact/folded screens: renders either the list or the detail full-screen.
 */
@Composable
fun FoldableListDetail(
    hasSelection: Boolean,
    listPane: @Composable () -> Unit,
    detailPane: @Composable () -> Unit,
    emptyDetailPane: @Composable () -> Unit,
    modifier: Modifier = Modifier,
) {
    val foldable = LocalFoldableState.current
    val colors = LocalFuseboxColors.current

    if (!foldable.supportsTwoPanes) {
        Box(modifier = modifier.fillMaxSize()) {
            if (hasSelection) detailPane() else listPane()
        }
        return
    }

    BoxWithConstraints(modifier = modifier.fillMaxSize()) {
        val totalDp = maxWidth.value
        val railWidthDp = if (foldable.useNavigationRail) 80f else 0f
        val hingeLeft = foldable.hingeBoundsDp?.left?.value?.minus(railWidthDp)
        val hingeRight = foldable.hingeBoundsDp?.right?.value?.minus(railWidthDp)

        val split = if (foldable.isHingeVertical && hingeLeft != null && hingeRight != null &&
            hingeLeft >= 240f && (totalDp - hingeRight) >= 260f
        ) {
            val gap = max(1f, hingeRight - hingeLeft)
            Triple((hingeLeft / (totalDp - gap)).coerceIn(0.35f, 0.55f), gap, true)
        } else {
            Triple(0.42f, 1f, false)
        }

        Row(modifier = Modifier.fillMaxSize()) {
            Box(modifier = Modifier.weight(split.first).fillMaxHeight()) {
                listPane()
            }
            if (split.third && split.second > 4f) {
                // Physical hinge gap between left and right halves on a foldable
                Box(
                    modifier = Modifier
                        .width(split.second.dp)
                        .fillMaxHeight()
                        .background(colors.bg),
                )
            } else {
                Box(
                    modifier = Modifier
                        .width(1.dp)
                        .fillMaxHeight()
                        .background(colors.line),
                )
            }
            Box(modifier = Modifier.weight(1f - split.first).fillMaxHeight()) {
                if (hasSelection) detailPane() else emptyDetailPane()
            }
        }
    }
}

/**
 * Spacer that respects a horizontal separating hinge on Flip / tabletop foldables,
 * keeping top and bottom sections off the crease when half-folded.
 */
@Composable
fun TabletopAwareSplit(
    top: @Composable () -> Unit,
    bottom: @Composable () -> Unit,
    modifier: Modifier = Modifier,
) {
    val foldable = LocalFoldableState.current
    if (foldable.isTabletopPosture && foldable.hingeBoundsDp != null) {
        val gap = foldable.hingeBoundsDp.height.coerceAtLeast(16.dp)
        Column(modifier = modifier.fillMaxSize()) {
            Box(modifier = Modifier.weight(1f).fillMaxWidth()) { top() }
            Spacer(Modifier.height(gap))
            Box(modifier = Modifier.weight(1f).fillMaxWidth()) { bottom() }
        }
    } else {
        Column(modifier = modifier.fillMaxWidth()) {
            top()
            bottom()
        }
    }
}
