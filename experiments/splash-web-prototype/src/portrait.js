import { passages } from './knowledge.js';

const clamp = (x) => Math.max(0, Math.min(1, x));

// Points on the shared artwork. Nearby regions receive fragments from the same gyre.
const knots = [
  { u: .678, v: .265, delay: .08, route: 1 }, // near eye
  { u: .748, v: .233, delay: .21, route: 2 }, // far eye
  { u: .692, v: .075, delay: .27, route: 1 }, // crown
  { u: .812, v: .151, delay: .16, route: 2 }, // bow
  { u: .603, v: .769, delay: .11, route: 3 }, // hand
  { u: .777, v: .526, delay: .26, route: 2 }, // bodice
  { u: .505, v: .435, delay: .19, route: 0 }, // high strands
  { u: .386, v: .655, delay: .39, route: 0 }, // low strands
  { u: .917, v: .615, delay: .30, route: 2 }, // far strands
  { u: .746, v: .887, delay: .20, route: 3 }, // drape
  { u: .900, v: .893, delay: .36, route: 3 }, // skirt
];

// Hair, ribbon and sleeve contours, traced in a 720 × 405 view of the shared artwork.
const contours = [
  [396, 130, 462, 118, 426, -14, 538, 17],
  [483, 31, 448, 64, 455, 109, 431, 146],
  [458, 117, 441, 161, 393, 199, 332, 169],
  [332, 169, 268, 143, 245, 211, 308, 229],
  [414, 211, 322, 273, 283, 196, 211, 258],
  [352, 247, 227, 249, 218, 317, 269, 344],
  [400, 324, 357, 367, 257, 344, 199, 388],
  [587, 75, 598, 102, 626, 105, 641, 129],
  [590, 154, 599, 213, 655, 211, 670, 248],
  [480, 167, 453, 150, 448, 220, 402, 260],
  [594, 259, 601, 320, 660, 333, 715, 354],
  [577, 291, 579, 340, 630, 372, 681, 404],
];

export function imagePlacement(image, width, height) {
  const fit = Math.max(width / image.naturalWidth, height / image.naturalHeight);
  const w = image.naturalWidth * fit, h = image.naturalHeight * fit;
  // Shared by the WebGL artwork and the CSS object-fit crop.
  return { x: (width - w) * .68, w, h };
}

function closestKnot(u, v) {
  let delay = 1, route = 0;
  for (const knot of knots) {
    const distance = Math.hypot((u - knot.u) * 1.52, (v - knot.v) * .83);
    const candidate = knot.delay + distance * 1.58;
    if (candidate < delay) {
      delay = candidate;
      route = knot.route;
    }
  }
  // Scatter local arrival times so none of the regions has a visible advancing edge.
  return { delay: clamp(delay + .071 * Math.sin(u * 72 + v * 45) * Math.cos(v * 61 - u * 17)), route };
}

function addContourCells(cells, letters, at, random) {
  for (const [index, points] of contours.entries()) {
    const [ax, ay, bx, by, cx, cy, dx, dy] = points;
    for (let t = random() * .05; t < 1;) {
      const s = 1 - t;
      const x = s ** 3 * ax + 3 * s * s * t * bx + 3 * s * t * t * cx + t ** 3 * dx;
      const y = s ** 3 * ay + 3 * s * s * t * by + 3 * s * t * t * cy + t ** 3 * dy;
      const tx = 3 * (s * s * (bx - ax) + 2 * s * t * (cx - bx) + t * t * (dx - cx));
      const ty = 3 * (s * s * (by - ay) + 2 * s * t * (cy - by) + t * t * (dy - cy));
      const speed = Math.max(1, Math.hypot(tx, ty));
      const entry = letters[Math.floor(random() * letters.length)];
      const glyphSize = .011 + random() * .004;
      const phase = index * .68 + t * 1.8;
      t += 405 * glyphSize * entry.ratio * (.85 + random() * .28) / Math.max(45, speed);
      // Find a nearby engraved stroke to keep the text aligned with the print.
      let u = x / 720, v = y / 405, strength = 0;
      for (const offset of [0, -2, 2, -4, 4]) {
        const uu = (x - offset * ty / speed) / 720, vv = (y + offset * tx / speed) / 405;
        const detail = at(uu, vv) - Math.abs(offset) * .044;
        if (detail > strength) {
          u = uu;
          v = vv;
          strength = detail;
        }
      }
      if (strength < .25) continue;
      const knot = closestKnot(u, v);
      const angle = Math.atan(Math.tan(Math.atan2(ty, tx))) * .87;
      cells.push({ u, v, glyphSize, entry, angle, weight: strength, tone: .72, eye: 0,
        route: knot.route, arrival: 1.48 + knot.delay * .93 + .26 * Math.sin(phase * 1.6) + random() * .20,
        thread: phase });
    }
  }
}

export function sampleArtwork(image, entries, random) {
  const width = 480, height = Math.round(width * image.naturalHeight / image.naturalWidth);
  const canvas = Object.assign(document.createElement('canvas'), { width, height });
  const ctx = canvas.getContext('2d', { willReadFrequently: true });
  ctx.drawImage(image, 0, 0, width, height);
  const pixels = ctx.getImageData(0, 0, width, height).data;
  const lum = new Float32Array(width * height);
  for (let i = 0; i < lum.length; i++) {
    lum[i] = (.45 * pixels[i * 4] + .46 * pixels[i * 4 + 1] + .09 * pixels[i * 4 + 2]) / 255;
  }
  const field = new Float32Array(lum.length);
  const texture = new Float32Array(lum.length);
  function luma(x, y) {
    return lum[Math.max(0, Math.min(height - 1, y)) * width + Math.max(0, Math.min(width - 1, x))];
  }
  for (let y = 0; y < height; y++) {
    const v = (y + .5) / height;
    for (let x = 0; x < width; x++) {
      const u = (x + .5) / width;
      const i = y * width + x;
      const local = luma(x, y);
      const surroundings = (luma(x - 3, y) + luma(x + 3, y) + luma(x, y - 3) + luma(x, y + 3)) * .25;
      const paper = .865 + .043 * Math.sin(Math.min(1, v * 1.28) * Math.PI * .5) - u * .021;
      const line = clamp((Math.abs(local - surroundings) - .004) * 15);
      const ink = clamp((paper - local - .012) * 7.3);
      const pearl = clamp((local - paper - .016) * 7.7);
      field[i] = clamp(Math.max(ink * .88, pearl * .80) + line * .58);
      texture[i] = line;
    }
  }
  const letters = entries.slice(passages.length);
  const short = letters.filter((entry) => entry.ratio <= 1.3);
  const cells = [];
  const aspect = image.naturalWidth / image.naturalHeight;
  for (let v = .004; v < .999;) {
    const finerRows = 1 - clamp(Math.abs(v - .255) / .135);
    const rowHeight = .0057 + random() * .0014 - finerRows * .0018;
    let u = random() * .004;
    while (u < .998) {
      const eye = 1 - clamp(Math.hypot((u - .719) / .078, (v - .253) / .098));
      const set = random() < eye * .92 ? short : letters;
      const entry = set[Math.floor(random() * set.length)];
      const glyphSize = (.0091 - .0032 * eye) * (.92 + random() * .19);
      const stride = glyphSize * (entry.ratio * .84 + .10) / aspect;
      const uu = Math.min(.999, u + stride * (.29 + random() * .46));
      const vv = Math.min(.999, v + rowHeight * (.24 + random() * .52));
      u += stride;
      const x = Math.floor(uu * width), y = Math.floor(vv * height), i = y * width + x;
      const weight = Math.max(field[i], eye * .67), line = texture[i];
      if (weight < .087 || random() > clamp(.21 + weight * 2.04 + eye * .36)) continue;
      const dx = luma(x + 2, y) - luma(x - 2, y), dy = luma(x, y + 2) - luma(x, y - 2);
      const angle = Math.atan(Math.tan(Math.atan2(-dx, dy))) * (.34 + .42 * line) * (1 - eye * .55);
      const knot = closestKnot(uu, vv);
      const tone = clamp((lum[i] - .758) / .179);
      cells.push({ u: uu, v: vv, glyphSize, entry, angle, weight, tone, eye, route: knot.route,
        arrival: 1.47 + knot.delay * .94 + random() * .62 - weight * .20 - eye * .16 });
    }
    v += rowHeight;
  }
  const contourLetters = letters.filter((entry) => entry.ratio >= .88 && entry.ratio <= 1.74);
  const fieldAt = (u, v) => field[Math.floor(clamp(v) * (height - 1)) * width + Math.floor(clamp(u) * (width - 1))];
  addContourCells(cells, contourLetters, fieldAt, random);

  // The support texture limits settled pigments to details that are present in the artwork.
  const support = ctx.createImageData(width, height);
  for (let i = 0; i < lum.length; i++) {
    const value = Math.round(field[i] * 255);
    support.data[i * 4] = value;
    support.data[i * 4 + 1] = value;
    support.data[i * 4 + 2] = value;
    support.data[i * 4 + 3] = 255;
  }
  ctx.putImageData(support, 0, 0);
  return { cells, support: canvas };
}
