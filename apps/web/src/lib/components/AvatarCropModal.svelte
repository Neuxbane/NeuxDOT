<script lang="ts">
import { loadImageFromFile, centeredSquare, clampCrop, cropAndCompress, type CropArea } from '$lib/image';
import { X, ZoomIn, Check, Loader2 } from 'lucide-svelte';

interface Props {
file: File;
/** Output square edge in px. */
size?: number;
/** Encoded byte ceiling. */
maxBytes?: number;
onCancel: () => void;
/** Called with the compressed data URL. */
onApply: (dataUrl: string) => void;
}

let { file, size = 256, maxBytes = 64 * 1024, onCancel, onApply }: Props = $props();

const VIEW = 320; // preview box edge in CSS px

let img = $state<HTMLImageElement | null>(null);
let error = $state('');
let busy = $state(false);
let dragging = $state(false);
// Offset from the pointer to the selection's top-left, in image pixels, so
// dragging moves the square naturally instead of snapping it to the cursor.
let grabDX = 0;
let grabDY = 0;

// Selection is stored in *image* pixels so it is resolution independent.
let area = $state<CropArea>({ x: 0, y: 0, width: 1, height: 1 });

$effect(() => {
let cancelled = false;
loadImageFromFile(file)
.then((loaded) => {
if (cancelled) return;
img = loaded;
area = centeredSquare(loaded.naturalWidth, loaded.naturalHeight);
})
.catch((e) => {
if (!cancelled) error = e?.message || 'Could not load image.';
});
return () => {
cancelled = true;
};
});

// Scale factor from image pixels -> preview CSS pixels.
// The image is rendered with `object-contain` inside a square host, so a
// non-square image is letterboxed. `scale` is uniform, but the drawn image
// is offset by (offsetX, offsetY) inside the host. Pointer coordinates must
// be converted through that offset, otherwise dragging is wrong (and can be
// clamped to the image edge) for any non-square aspect ratio.
const scale = $derived(img ? VIEW / Math.max(img.naturalWidth, img.naturalHeight) : 1);

const drawnW = $derived(img ? img.naturalWidth * scale : VIEW);
const drawnH = $derived(img ? img.naturalHeight * scale : VIEW);
const offsetX = $derived((VIEW - drawnW) / 2);
const offsetY = $derived((VIEW - drawnH) / 2);

const selStyle = $derived(
img
? `left:${offsetX + area.x * scale}px;top:${offsetY + area.y * scale}px;width:${area.width * scale}px;height:${area.height * scale}px;`
: ''
);

function startDrag(e: PointerEvent) {
if (!img) return;
const pos = pointerToImage(e);
if (!pos) return;
// If the pointer is inside the current square, keep the grab offset;
// otherwise start a fresh drag from the square's centre.
const inside =
pos.x >= area.x &&
pos.x <= area.x + area.width &&
pos.y >= area.y &&
pos.y <= area.y + area.height;
if (inside) {
grabDX = pos.x - area.x;
grabDY = pos.y - area.y;
} else {
grabDX = area.width / 2;
grabDY = area.height / 2;
}
dragging = true;
(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
moveTo(e);
}

/** Converts a pointer event into image-pixel coordinates. */
function pointerToImage(e: PointerEvent): { x: number; y: number } | null {
if (!img) return null;
const host = (e.currentTarget as HTMLElement).closest('[data-crop-host]') as HTMLElement | null;
if (!host) return null;
const rect = host.getBoundingClientRect();
// The host may be visually scaled by CSS; map through its measured size.
const hostScale = rect.width / VIEW || 1;
const pxInHost = (e.clientX - rect.left) / hostScale;
const pyInHost = (e.clientY - rect.top) / hostScale;
return {
x: (pxInHost - offsetX) / scale,
y: (pyInHost - offsetY) / scale
};
}

function moveTo(e: PointerEvent) {
const pos = pointerToImage(e);
if (!pos || !img) return;
const side = area.width;
area = clampCrop(
{ x: pos.x - grabDX, y: pos.y - grabDY, width: side, height: side },
img.naturalWidth,
img.naturalHeight
);
}

function onMove(e: PointerEvent) {
if (!dragging) return;
moveTo(e);
}

function endDrag() {
dragging = false;
}

function setZoom(percent: number) {
if (!img) return;
const maxSide = Math.min(img.naturalWidth, img.naturalHeight);
const side = Math.max(16, (maxSide * percent) / 100);
const cx = area.x + area.width / 2;
const cy = area.y + area.height / 2;
area = clampCrop(
{ x: cx - side / 2, y: cy - side / 2, width: side, height: side },
img.naturalWidth,
img.naturalHeight
);
}

const zoomPercent = $derived(
img ? Math.round((area.width / Math.min(img.naturalWidth, img.naturalHeight)) * 100) : 100
);

async function apply() {
if (!img) return;
busy = true;
error = '';
try {
const { dataUrl } = await cropAndCompress(img, area, { size, maxBytes });
onApply(dataUrl);
} catch (e: any) {
error = e?.message || 'Could not process the image.';
} finally {
busy = false;
}
}

function onKey(e: KeyboardEvent) {
if (e.key === 'Escape' && !busy) onCancel();
}
</script>

<svelte:window on:keydown={onKey} />

<div class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm p-4" role="dialog" aria-modal="true" aria-label="Crop profile picture">
<div class="w-full max-w-md rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900 shadow-2xl">
<div class="flex items-center justify-between border-b border-zinc-200/70 dark:border-zinc-800 px-4 py-3">
<h2 class="text-sm font-semibold text-zinc-900 dark:text-white">Crop profile picture</h2>
<button type="button" onclick={onCancel} disabled={busy} class="rounded-lg p-1 text-zinc-400 hover:text-zinc-900 dark:hover:text-white transition-colors cursor-pointer" aria-label="Cancel">
<X class="h-4 w-4" />
</button>
</div>

<div class="p-4 space-y-3">
{#if error}
<p class="rounded-lg bg-rose-500/10 px-3 py-2 text-[11px] text-rose-600 dark:text-rose-400">{error}</p>
{/if}

<div
data-crop-host
class="relative mx-auto overflow-hidden rounded-xl bg-zinc-100 dark:bg-zinc-950 select-none touch-none"
style="width:{VIEW}px;height:{VIEW}px;"
onpointerdown={startDrag}
onpointermove={onMove}
onpointerup={endDrag}
onpointercancel={endDrag}
role="application"
aria-label="Drag to reposition the square crop area"
>
{#if img}
<img src={img.src} alt="" class="pointer-events-none absolute inset-0 h-full w-full object-contain" draggable="false" />
<!-- Dim everything outside the square -->
<div class="pointer-events-none absolute inset-0" style="box-shadow: 0 0 0 9999px rgba(0,0,0,0.55);"></div>
<div
class="pointer-events-none absolute border-2 border-white shadow-[0_0_0_1px_rgba(0,0,0,0.4)] cursor-move {dragging ? 'border-cyan-400' : ''}"
style={selStyle}
></div>
{:else}
<div class="flex h-full items-center justify-center text-xs text-zinc-400">
<Loader2 class="mr-2 h-4 w-4 animate-spin" /> Loading…
</div>
{/if}
</div>

<div class="flex items-center gap-2">
<ZoomIn class="h-3.5 w-3.5 shrink-0 text-zinc-400" />
<input
type="range"
min="20"
max="100"
value={zoomPercent}
oninput={(e) => setZoom(Number((e.currentTarget as HTMLInputElement).value))}
class="flex-1 accent-cyan-500"
aria-label="Crop size"
/>
<span class="w-10 text-right text-[10px] font-mono text-zinc-400">{zoomPercent}%</span>
</div>

<p class="text-[10px] text-zinc-400 dark:text-zinc-500">
Drag the square to choose the area. The result is square, downscaled to {size}×{size} and compressed automatically.
</p>
</div>

<div class="flex items-center justify-end gap-2 border-t border-zinc-200/70 dark:border-zinc-800 px-4 py-3">
<button type="button" onclick={onCancel} disabled={busy} class="rounded-xl px-3 py-1.5 text-xs font-medium text-zinc-600 dark:text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors cursor-pointer">
Cancel
</button>
<button
type="button"
onclick={apply}
disabled={busy || !img}
class="flex items-center gap-1.5 rounded-xl bg-zinc-900 hover:bg-zinc-800 dark:bg-white dark:hover:bg-zinc-100 px-3 py-1.5 text-xs font-medium text-white dark:text-zinc-950 shadow-xs transition-all disabled:opacity-50 cursor-pointer"
>
{#if busy}
<Loader2 class="h-3.5 w-3.5 animate-spin" />
{:else}
<Check class="h-3.5 w-3.5" />
{/if}
<span>Apply</span>
</button>
</div>
</div>
</div>
