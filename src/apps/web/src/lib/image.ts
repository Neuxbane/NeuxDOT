/**
 * Image helpers: square cropping + adaptive compression.
 *
 * Avatars are stored inline in SQLite as data URLs, so the goal is the
 * *smallest* payload that still looks sharp at display size. Output is always
 * a square, center-cropped from the user's selection, downscaled to a modest
 * resolution, and encoded as WebP (JPEG fallback) at the best quality that
 * stays under a byte budget.
 */

export interface CropArea {
/** Source x offset in pixels (of the loaded image). */
x: number;
y: number;
width: number;
height: number;
}

export interface CompressOptions {
/** Target edge length in px for the output square. */
size?: number;
/** Hard ceiling for the encoded byte size. */
maxBytes?: number;
/** Quality search range. */
minQuality?: number;
maxQuality?: number;
}

const DEFAULTS: Required<CompressOptions> = {
size: 256,
maxBytes: 64 * 1024,
minQuality: 0.4,
maxQuality: 0.92
};

/** Loads a File/Blob into an HTMLImageElement. */
export function loadImageFromFile(file: File): Promise<HTMLImageElement> {
return new Promise((resolve, reject) => {
const url = URL.createObjectURL(file);
const img = new Image();
img.onload = async () => {
URL.revokeObjectURL(url);
try {
resolve(await downscaleIfHuge(img));
} catch {
// Never fail the upload just because pre-scaling failed; the crop step
// will handle the original.
resolve(img);
}
};
img.onerror = () => {
URL.revokeObjectURL(url);
reject(new Error('Could not decode that image.'));
};
img.src = url;
});
}

/**
 * Largest edge we are willing to keep in memory while cropping. Modern phone
 * photos are frequently 8-12k px on the long edge (~100+ MB decoded); scaling
 * them down first keeps the canvas operations fast and avoids browser memory
 * failures. The final avatar is 256 px, so this costs no visible quality.
 */
export const MAX_IMAGE_EDGE = 2048;

/** Downscales an oversized image, returning the original when within bounds. */
async function downscaleIfHuge(img: HTMLImageElement): Promise<HTMLImageElement> {
const w = img.naturalWidth || img.width;
const h = img.naturalHeight || img.height;
const longest = Math.max(w, h);
if (longest <= MAX_IMAGE_EDGE) return img;

const ratio = MAX_IMAGE_EDGE / longest;
const canvas = document.createElement('canvas');
canvas.width = Math.max(1, Math.round(w * ratio));
canvas.height = Math.max(1, Math.round(h * ratio));
const ctx = canvas.getContext('2d');
if (!ctx) return img;
ctx.imageSmoothingEnabled = true;
ctx.imageSmoothingQuality = 'high';
ctx.drawImage(img, 0, 0, canvas.width, canvas.height);

const dataUrl = canvas.toDataURL('image/png');
return await new Promise<HTMLImageElement>((resolve, reject) => {
const scaled = new Image();
scaled.onload = () => resolve(scaled);
scaled.onerror = () => reject(new Error('Could not prepare the image.'));
scaled.src = dataUrl;
});
}

/** Clamps a crop area to the image bounds and forces it square. */
export function clampCrop(area: CropArea, imageW: number, imageH: number): CropArea {
const side = Math.max(1, Math.min(area.width, area.height, imageW, imageH));
const x = Math.min(Math.max(0, area.x), Math.max(0, imageW - side));
const y = Math.min(Math.max(0, area.y), Math.max(0, imageH - side));
return { x, y, width: side, height: side };
}

/** Largest centered square that fits inside the image. */
export function centeredSquare(imageW: number, imageH: number): CropArea {
const side = Math.min(imageW, imageH);
return { x: (imageW - side) / 2, y: (imageH - side) / 2, width: side, height: side };
}

function canvasToBlob(canvas: HTMLCanvasElement, type: string, quality: number): Promise<Blob | null> {
return new Promise((resolve) => canvas.toBlob(resolve, type, quality));
}

function blobToDataUrl(blob: Blob): Promise<string> {
return new Promise((resolve, reject) => {
const reader = new FileReader();
reader.onload = () => resolve(String(reader.result));
reader.onerror = () => reject(new Error('Could not encode the image.'));
reader.readAsDataURL(blob);
});
}

/**
 * Renders `area` of `img` into a square `size`x`size` canvas, then binary
 * searches the encoder quality for the smallest output that satisfies
 * `maxBytes` while staying as sharp as possible.
 *
 * Returns a data URL suitable for `avatar_url`.
 */
export async function cropAndCompress(
img: HTMLImageElement,
area: CropArea,
options: CompressOptions = {}
): Promise<{ dataUrl: string; bytes: number; quality: number; type: string }> {
const cfg = { ...DEFAULTS, ...options };
const crop = clampCrop(area, img.naturalWidth || img.width, img.naturalHeight || img.height);

// Downscale only; never upscale a small image (that would add bytes and
// blur without adding detail).
const edge = Math.max(16, Math.min(cfg.size, Math.round(crop.width)));

const canvas = document.createElement('canvas');
canvas.width = edge;
canvas.height = edge;
const ctx = canvas.getContext('2d');
if (!ctx) throw new Error('Canvas is not available in this browser.');
ctx.imageSmoothingEnabled = true;
ctx.imageSmoothingQuality = 'high';
ctx.drawImage(
img,
crop.x,
crop.y,
crop.width,
crop.height,
0,
0,
edge,
edge
);

// Prefer WebP (roughly 25-35% smaller than JPEG at equal quality).
const preferred = 'image/webp';
const fallback = 'image/jpeg';

const supportsWebp = await new Promise<boolean>((resolve) => {
canvas.toBlob((b) => resolve(!!b && b.type === preferred), preferred, 0.8);
});
const type = supportsWebp ? preferred : fallback;

// Find the highest quality that fits the byte budget.
let lo = cfg.minQuality;
let hi = cfg.maxQuality;
let best: { blob: Blob; quality: number } | null = null;

// Fast path: if the highest quality already fits, use it.
const top = await canvasToBlob(canvas, type, hi);
if (top && top.size <= cfg.maxBytes) {
best = { blob: top, quality: hi };
} else {
for (let i = 0; i < 6; i++) {
const mid = (lo + hi) / 2;
const blob = await canvasToBlob(canvas, type, mid);
if (!blob) break;
if (blob.size <= cfg.maxBytes) {
best = { blob, quality: mid };
lo = mid; // try to get sharper
} else {
hi = mid; // too heavy, back off
}
}
}

// Nothing fit at the minimum quality -> last resort: re-encode a smaller
// square so the payload stays bounded.
if (!best) {
const shrunk = document.createElement('canvas');
const small = Math.max(16, Math.round(edge / 2));
shrunk.width = small;
shrunk.height = small;
const sctx = shrunk.getContext('2d');
if (!sctx) throw new Error('Canvas is not available in this browser.');
sctx.imageSmoothingEnabled = true;
sctx.imageSmoothingQuality = 'high';
sctx.drawImage(canvas, 0, 0, small, small);
const blob = await canvasToBlob(shrunk, type, cfg.minQuality);
if (!blob) throw new Error('Could not compress the image.');
best = { blob, quality: cfg.minQuality };
}

const dataUrl = await blobToDataUrl(best.blob);
return { dataUrl, bytes: best.blob.size, quality: best.quality, type };
}
