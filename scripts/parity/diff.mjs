// A perceptual image diff for the parity harness, in the manner of pixelmatch. Two RGBA images are
// compared pixel by pixel as YIQ colour distances. A pixel that differs only at an anti-aliased edge
// (in either image) is not counted. Images of different sizes are compared at their smaller common
// size, each resampled by nearest neighbour.

// The YIQ distance between black and white, as pixelmatch scales it: a threshold of 1 is the largest.
const MAX_DELTA_SCALE = 35215;

/** Nearest-neighbour resample of an RGBA image to width x height. */
export function resize(image, width, height) {
  if (image.width === width && image.height === height) return image;
  const data = new Uint8Array(width * height * 4);
  for (let y = 0; y < height; y += 1) {
    const sy = Math.min(image.height - 1, Math.floor(((y + 0.5) * image.height) / height));
    for (let x = 0; x < width; x += 1) {
      const sx = Math.min(image.width - 1, Math.floor(((x + 0.5) * image.width) / width));
      const s = (sy * image.width + sx) * 4;
      data.set(image.data.subarray(s, s + 4), (y * width + x) * 4);
    }
  }
  return { width, height, data };
}

/** A channel blended over white by its alpha, as pixelmatch does for translucent pixels. */
function over(channel, alpha) {
  return 255 + (channel - 255) * alpha;
}

/** The YIQ components of the pixel at offset o. */
function yiq(data, o) {
  const alpha = data[o + 3] / 255;
  const r = over(data[o], alpha);
  const g = over(data[o + 1], alpha);
  const b = over(data[o + 2], alpha);
  return [
    r * 0.29889531 + g * 0.58662247 + b * 0.11448223,
    r * 0.59597799 - g * 0.2741761 - b * 0.32180189,
    r * 0.21147017 - g * 0.52261711 + b * 0.31114694,
  ];
}

/** The YIQ distance between two pixels, 0 when they are the same colour. */
function distance(a, ao, b, bo) {
  if (
    a[ao] === b[bo] && a[ao + 1] === b[bo + 1] && a[ao + 2] === b[bo + 2] && a[ao + 3] === b[bo + 3]
  ) {
    return 0;
  }
  const [y1, i1, q1] = yiq(a, ao);
  const [y2, i2, q2] = yiq(b, bo);
  const dy = y1 - y2;
  const di = i1 - i2;
  const dq = q1 - q2;
  return 0.5053 * dy * dy + 0.299 * di * di + 0.1957 * dq * dq;
}

/**
 * True when the pixel at x, y sits on an anti-aliased edge: at most two of its eight neighbours
 * share its colour (within maxDelta). A pixel inside a flat area or a solid block has more.
 */
function antialiased(image, x, y, maxDelta) {
  const { width, height, data } = image;
  const o = (y * width + x) * 4;
  let same = 0;
  for (let dy = -1; dy <= 1; dy += 1) {
    for (let dx = -1; dx <= 1; dx += 1) {
      if (dx === 0 && dy === 0) continue;
      const nx = x + dx;
      const ny = y + dy;
      if (nx < 0 || ny < 0 || nx >= width || ny >= height) continue;
      if (distance(data, o, data, (ny * width + nx) * 4) <= maxDelta) same += 1;
      if (same > 2) return false;
    }
  }
  return true;
}

/**
 * Compares two RGBA images ({width, height, data}). `threshold` (0 to 1, default 0.1) is the colour
 * tolerance: 0 is exact, 1 accepts anything. Returns the mismatched pixel count, the total, the ratio
 * (mismatched / total), the input sizes, and a diff image: mismatches red, anti-aliased differences
 * yellow, every other pixel a faint grey of the first image.
 */
export function diff(a, b, { threshold = 0.1 } = {}) {
  const width = Math.min(a.width, b.width);
  const height = Math.min(a.height, b.height);
  const first = resize(a, width, height);
  const second = resize(b, width, height);
  const maxDelta = MAX_DELTA_SCALE * threshold * threshold;
  const out = new Uint8Array(width * height * 4);
  let mismatched = 0;
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      const o = (y * width + x) * 4;
      if (distance(first.data, o, second.data, o) <= maxDelta) {
        const grey = Math.round(over(yiq(first.data, o)[0], 0.1));
        out.set([grey, grey, grey, 255], o);
      } else if (antialiased(first, x, y, maxDelta) || antialiased(second, x, y, maxDelta)) {
        out.set([255, 255, 0, 255], o);
      } else {
        mismatched += 1;
        out.set([255, 0, 0, 255], o);
      }
    }
  }
  const total = width * height;
  return {
    mismatched,
    total,
    ratio: total === 0 ? 0 : mismatched / total,
    sizes: { a: [a.width, a.height], b: [b.width, b.height] },
    image: { width, height, data: out },
  };
}
