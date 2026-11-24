import { passages, fragments } from './knowledge.js';
import { screenVertex, atmosphereFragment, glyphVertex, glyphFragment } from './shaders.js';
import { imagePlacement, sampleArtwork } from './portrait.js';

const TAU = Math.PI * 2;
const STRIDE = 16;
const DEPTH_BUCKETS = 16;
const clamp = (x, lo = 0, hi = 1) => Math.min(hi, Math.max(lo, x));
const mix = (a, b, t) => a + (b - a) * t;
const wrap = (x) => x - Math.floor(x);
const smooth = (a, b, x) => {
  const p = clamp((x - a) / (b - a));
  return p * p * (3 - 2 * p);
};

export const handoffProgress = (time, duration) => smooth(3.28, duration, time);

function randomGenerator(seed) {
  return () => {
    seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
    return seed / 4294967296;
  };
}

function makeAtlas() {
  const canvas = Object.assign(document.createElement('canvas'), { width: 2048, height: 1024 });
  const ctx = canvas.getContext('2d');
  ctx.fillStyle = '#000';
  ctx.fillRect(0, 0, 2048, 1024);
  ctx.font = '25px Consolas, "Microsoft YaHei", "Meiryo", monospace';
  ctx.textBaseline = 'middle';
  ctx.fillStyle = '#fff';
  let x = 4, y = 4;
  const height = 40;
  const entries = [...passages, ...fragments].map((word) => {
    const width = Math.ceil(ctx.measureText(word).width) + 14;
    if (x + width + 4 > 2048) {
      x = 4;
      y += height + 7;
    }
    ctx.fillText(word, x + 7, y + height / 2 + 1);
    const rect = [x / 2048, y / 1024, (x + width) / 2048, (y + height) / 1024];
    x += width + 8;
    return { rect, ratio: width / height };
  });
  return { canvas, entries };
}

function program(gl, vertex, fragment, uniforms) {
  const id = gl.createProgram();
  for (const [type, source] of [[gl.VERTEX_SHADER, vertex], [gl.FRAGMENT_SHADER, fragment]]) {
    const shader = gl.createShader(type);
    gl.shaderSource(shader, source);
    gl.compileShader(shader);
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(shader));
    gl.attachShader(id, shader);
    gl.deleteShader(shader);
  }
  gl.linkProgram(id);
  if (!gl.getProgramParameter(id, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(id));
  return { id, ...Object.fromEntries(uniforms.map((name) => [name, gl.getUniformLocation(id, name)])) };
}

function bindTexture(gl, source, slot, mipmaps) {
  gl.activeTexture(gl.TEXTURE0 + slot);
  gl.bindTexture(gl.TEXTURE_2D, gl.createTexture());
  gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, source);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, mipmaps ? gl.LINEAR_MIPMAP_LINEAR : gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  if (mipmaps) gl.generateMipmap(gl.TEXTURE_2D);
}

const gyres = [
  { x: .26, y: .38, u: .43, v: .54, scale: .89, start: .27, spin: 1, flatten: .45, tilt: .14, density: .92 },
  { x: .51, y: .22, u: .64, v: .18, scale: .66, start: .39, spin: -1, flatten: .31, tilt: -.09, density: .82 },
  { x: .79, y: .43, u: .82, v: .36, scale: 1.02, start: .48, spin: 1, flatten: .54, tilt: .17, density: .87 },
  { x: .56, y: .70, u: .61, v: .74, scale: .75, start: .35, spin: -1, flatten: .37, tilt: -.23, density: .75 },
];

function seaCrest(lane, x, time) {
  return .51 + lane * .10 - .12 * (x - .5) + Math.sin(x * 6.7 - time * .42 - lane * .62) * (.079 + lane * .022)
    + Math.sin(x * 13.1 + time * .32 + lane * 1.33) * .031;
}

function makeCurrent(entries, cells) {
  const random = randomGenerator(148046);
  const count = 2100 + cells.length;
  return Array.from({ length: count }, (_, i) => {
    const cell = cells[i - 2100];
    const front = i < 24;
    const ribbon = !cell && i < 435;
    const bridge = i >= 435 && i < 615;
    const entry = cell?.entry ?? entries[Math.floor(random() * (ribbon ? passages.length : entries.length))];
    const lane = Math.floor(random() * 5), start = random(), depth = random();
    const isAir = random() < .34, brightness = random();
    const route = cell?.route ?? Math.floor(random() * gyres.length);
    return { cell, entry, lane, start, depth, isAir, brightness, route, orbit: random(), phase: random() * TAU, front, ribbon, bridge,
      scale: random(), motion: random(), drift: random(), free: !cell && !bridge && random() < .48,
      linger: random() < .28, tone: random(), thread: random() };
  });
}

export function createScene(canvas, { duration, background }) {
  const gl = canvas.getContext('webgl2', { alpha: true, antialias: false, depth: false, powerPreference: 'high-performance' });
  if (!gl) throw new Error('此浏览器当前无法使用 WebGL 2。');
  const atlas = makeAtlas();
  const { cells, support } = sampleArtwork(background, atlas.entries, randomGenerator(13491));
  const current = makeCurrent(atlas.entries, cells);
  const instances = new Float32Array(current.length * STRIDE);
  const sorted = new Float32Array(instances.length);
  const layers = new Uint8Array(current.length);
  const layerSizes = new Uint16Array(DEPTH_BUCKETS);
  const layerOffsets = new Uint16Array(DEPTH_BUCKETS);
  const centerData = new Float32Array(4 * gyres.length);
  const empty = gl.createVertexArray();
  const glyphs = gl.createVertexArray();
  const buffer = gl.createBuffer();
  gl.bindVertexArray(glyphs);
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  gl.bufferData(gl.ARRAY_BUFFER, instances.byteLength, gl.DYNAMIC_DRAW);
  for (let i = 0; i < 4; i++) {
    gl.enableVertexAttribArray(i);
    gl.vertexAttribPointer(i, 4, gl.FLOAT, false, STRIDE * 4, i * 16);
    gl.vertexAttribDivisor(i, 1);
  }
  const sky = program(gl, screenVertex, atmosphereFragment, ['size', 'time', 'formed', 'wake', 'dawn', 'cores[0]']);
  const glyph = program(gl, glyphVertex, glyphFragment, ['size', 'letters', 'artwork', 'support', 'pictureBounds', 'daylight']);
  bindTexture(gl, atlas.canvas, 0, true);
  bindTexture(gl, background, 1, true);
  bindTexture(gl, support, 2, false);
  gl.useProgram(glyph.id);
  gl.uniform1i(glyph.letters, 0);
  gl.uniform1i(glyph.artwork, 1);
  gl.uniform1i(glyph.support, 2);
  gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
  gl.clearColor(0, 0, 0, 0);

  let width, height, picture;
  function resize() {
    const box = background.getBoundingClientRect();
    width = Math.max(1, box.width);
    height = Math.max(1, box.height);
    picture = imagePlacement(background, width, height);
    const density = Math.min(devicePixelRatio || 1, 1.65, 2048 / width, 1250 / height);
    canvas.width = Math.max(1, Math.round(width * density));
    canvas.height = Math.max(1, Math.round(height * density));
    gl.viewport(0, 0, canvas.width, canvas.height);
  }

  function drawGlyphs(count, dawn) {
    if (!count) return;
    let offset = 0;
    for (let i = 0; i < DEPTH_BUCKETS; i++) {
      layerOffsets[i] = offset;
      offset += layerSizes[i];
    }
    for (let i = 0, source = 0; i < count; i++, source += STRIDE) {
      const target = layerOffsets[layers[i]]++ * STRIDE;
      for (let j = 0; j < STRIDE; j++) sorted[target + j] = instances[source + j];
    }
    gl.useProgram(glyph.id);
    gl.uniform2f(glyph.size, width, height);
    gl.uniform4f(glyph.pictureBounds, picture.x / width, 0, picture.w / width, picture.h / height);
    gl.uniform1f(glyph.daylight, dawn);
    gl.bindVertexArray(glyphs);
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.bufferSubData(gl.ARRAY_BUFFER, 0, sorted, 0, count * STRIDE);
    gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, count);
  }

  function draw(time) {
    const dawn = handoffProgress(time, duration);
    if (dawn >= 1) {
      gl.clear(gl.COLOR_BUFFER_BIT);
      return;
    }
    const skyDepth = height * .0042 + 3;
    const coherent = smooth(1.35, 2.56, time);
    const tide = time * 2.45;
    const orbitZoom = .022 * Math.sin(tide * .4);
    const seaFade = 1 - smooth(.02, .71, dawn);
    const letterFade = 1 - smooth(.31, .98, dawn);
    const heading = smooth(.72, 1.91, time) * .42;
    for (let i = 0; i < gyres.length; i++) {
      const g = gyres[i];
      centerData[4 * i] = mix(g.x + Math.sin(tide * .27 + i * 1.5) * .022, clamp((picture.x + g.u * picture.w) / width, .14, .88), heading);
      centerData[4 * i + 1] = mix(g.y + Math.cos(tide * .34 + i * 1.8) * .027, clamp(g.v * picture.h / height, .14, .83), heading);
      centerData[4 * i + 2] = smooth(g.start, g.start + .69, time) * g.density;
      centerData[4 * i + 3] = g.spin;
    }

    gl.disable(gl.BLEND);
    gl.bindVertexArray(empty);
    gl.useProgram(sky.id);
    gl.uniform2f(sky.size, width, height);
    gl.uniform1f(sky.time, tide);
    gl.uniform1f(sky.formed, coherent);
    gl.uniform1f(sky.wake, 1 - .86 * smooth(.42, 1.32, time));
    gl.uniform1f(sky.dawn, dawn);
    gl.uniform4fv(sky['cores[0]'], centerData);
    gl.drawArrays(gl.TRIANGLES, 0, 3);

    gl.enable(gl.BLEND);
    layerSizes.fill(0);
    let count = 0;
    for (const p of current) {
      const { cell, entry, depth, phase, route } = p;
      if (cell && letterFade < .009) continue;
      const contour = cell?.thread !== undefined;
      const onward = cell ? smooth(cell.arrival - .54, cell.arrival + .57, time) : 0;
      const g = gyres[route];
      const d = depth * depth;
      let x = 0, y = 0, font = 0, blur = 0, angle = 0, opacity = 0, red = 0, green = 0, blue = 0;
      let layer = .05 + .63 * d;
      if (onward < 1) {
        const flow = wrap(p.start + tide * (.019 + d * .049) * (p.lane % 2 ? -1 : 1));
        const strand = flow * TAU;
        const wave = Math.sin(strand * .71 + p.lane * 1.46 + tide * .20);
        const crossing = Math.cos(strand * 1.54 - tide * .27 + p.lane * .37);
        const streamX = .5 + (flow - .5) * mix(1.25, 1.91, d) + .033 * crossing * (1 + depth);
        const streamY = p.isAir
          ? .078 + depth * .48 + wave * .062 + crossing * .043
          : .407 + d * .62 + wave * (.041 + d * .13) + crossing * .048 + (p.lane - 2) * .024;
        const edge = smooth(0, .14, flow) * (1 - smooth(.86, 1, flow));
        const glint = Math.pow(.5 + .5 * Math.sin(tide * (.72 + p.motion) + phase), 5);
        x = streamX * width;
        y = (streamY + Math.sin(tide * .48 + phase) * (.005 + d * .008)) * height;
        font = skyDepth * (.62 + d * (1.65 + p.scale * 1.34));
        blur = clamp((d - .67) * 2.5 + (p.isAir ? .16 : 0));
        angle = (crossing * .114 + wave * .074) * (p.isAir ? .6 : 1);
        opacity = (.15 + p.brightness * .51 + glint * .21) * edge * (p.isAir ? .62 : 1);
        red = .16 + p.tone * .18 + glint * .08;
        green = .50 + p.tone * .34 + glint * .11;
        blue = .33 + p.tone * .21 + glint * .08;
        if (p.ribbon) {
          const lane = p.lane % 4, u = flow * 1.43 - .215;
          x = u * width;
          y = height * (seaCrest(lane, u, tide) + (p.orbit - .5) * (.025 + lane * .009));
          angle = Math.atan2((seaCrest(lane, u + .01, tide) - seaCrest(lane, u, tide)) * height, .01 * width) * .70;
          font = skyDepth * (p.front ? 2.0 + p.scale * 1.43 : .66 + p.scale * 1.02 + lane * .28);
          opacity = edge * (p.front ? .48 + glint * .28 : .19 + p.brightness * .29 + glint * .25);
          blur = p.front ? .025 : .11;
        }

        const drawIn = p.free ? 0 : smooth(g.start + p.drift * .16, g.start + .62 + p.drift * .17, time) * (cell ? 1 : 1 - coherent * .86);
        if (drawIn > 0) {
          const orbit = contour ? .54 + p.orbit * .16 : Math.pow(p.orbit, 1.37);
          const radius = (height * .37 + width * .041) * g.scale * (.10 + orbit * .93 + orbitZoom);
          const turn = (contour ? cell.thread + phase * .07 : phase)
            + g.spin * (tide * (.66 + (1 - orbit) * .96) + p.thread * (contour ? .28 : .8));
          const curl = turn + Math.sin(p.thread * TAU + tide * .24) * .24;
          const bend = Math.sin(curl * 2 + route) * .14;
          let whirlX = width * centerData[route * 4] + Math.cos(curl) * radius * (1 + bend);
          let whirlY = height * centerData[route * 4 + 1] + Math.sin(curl) * radius * g.flatten + Math.cos(curl) * radius * g.tilt;
          if (p.bridge) {
            const other = (route + (route % 2 === 0 ? 1 : 2)) % gyres.length;
            const travel = wrap(p.start + tide * .127);
            const arc = Math.sin(travel * Math.PI);
            whirlX = width * mix(centerData[route * 4], centerData[other * 4], travel) + arc * height * (.10 * g.spin + bend * .25);
            whirlY = height * (mix(centerData[route * 4 + 1], centerData[other * 4 + 1], travel) + arc * (.17 * g.spin + bend * .35));
          }
          x = mix(x, whirlX, drawIn);
          y = mix(y, whirlY, drawIn);
          font = mix(font, skyDepth * (.68 + d * 1.20), drawIn);
          const tangent = Math.atan2(Math.cos(curl) * g.flatten - Math.sin(curl) * g.tilt, -Math.sin(curl));
          angle = mix(angle, Math.atan(Math.tan(tangent)) * .40, drawIn);
          opacity = mix(opacity, (.26 + p.brightness * .45 + glint * .25) * g.density, drawIn);
          blur *= 1 - .7 * drawIn;
        }
      }

      if (cell) {
        const targetX = picture.x + cell.u * picture.w;
        const targetY = cell.v * picture.h;
        if (onward === 1) {
          x = targetX;
          y = targetY;
          angle = cell.angle;
        } else if (onward > 0) {
          // Bend away from the gyre, then approach along the contour of the engraving.
          const dx = targetX - x, dy = targetY - y;
          const tx = Math.cos(cell.angle), ty = Math.sin(cell.angle);
          const distance = Math.hypot(dx, dy);
          const direction = clamp((dx * tx + dy * ty) / (1 + distance * .33), -1, 1);
          const remaining = 1 - onward;
          const bend = onward * remaining * remaining * g.spin * (.94 + p.orbit * .82);
          const approach = onward * onward * remaining * direction
            * Math.min(distance * .82, height * .4) * (contour ? 1.28 : .65);
          x += dx * onward - dy * bend - tx * approach;
          y += dy * onward + dx * bend - ty * approach;
          const flowAngle = Math.atan(Math.tan(Math.atan2(dy + dx * bend * 4, dx - dy * bend * 4)));
          angle = mix(angle, mix(flowAngle * .71, cell.angle, smooth(.26, .98, onward)), onward);
        }
        const flash = 4 * onward * (1 - onward);
        font = mix(font, picture.h * cell.glyphSize, onward) * (1 + flash * (contour ? .23 : .05));
        blur = mix(blur, .026, onward);
        const tone = smooth(.06, .93, cell.tone);
        const gleam = flash * (contour ? .17 : .038) * (1 - cell.eye * .72);
        red = mix(red, .07 + .69 * tone + gleam, onward);
        green = mix(green, .20 + .76 * tone + gleam, onward);
        blue = mix(blue, .15 + .67 * tone + gleam, onward);
        const contourGleam = contour ? smooth(.51, .94, onward) * (1 - smooth(cell.arrival + .48, cell.arrival + .94, time)) : 0;
        opacity = mix(opacity, .28 + cell.weight * .44 + .26 * tone + contourGleam * .19, onward) * letterFade;
        layer = mix(layer, .49 + .11 * onward, onward);
      } else {
        opacity *= (1 - coherent * (p.linger ? .49 : .80)) * seaFade;
      }

      const glyphWidth = font * entry.ratio;
      if (opacity < .018 || x < -glyphWidth || x > width + glyphWidth || y < -font || y > height + font) continue;
      const bucket = Math.floor(clamp(layer, 0, .9999) * DEPTH_BUCKETS);
      layerSizes[bucket]++;
      layers[count] = bucket;
      const index = count++ * STRIDE;
      const rect = entry.rect;
      instances[index] = x;
      instances[index + 1] = y;
      instances[index + 2] = glyphWidth;
      instances[index + 3] = font;
      instances[index + 4] = rect[0];
      instances[index + 5] = rect[1];
      instances[index + 6] = rect[2];
      instances[index + 7] = rect[3];
      instances[index + 8] = clamp(red);
      instances[index + 9] = clamp(green);
      instances[index + 10] = clamp(blue);
      instances[index + 11] = clamp(opacity);
      instances[index + 12] = angle;
      instances[index + 13] = blur;
      instances[index + 14] = onward;
      instances[index + 15] = contour ? 1 : 0;
    }
    drawGlyphs(count, dawn);
  }

  resize();
  return { draw, resize };
}
