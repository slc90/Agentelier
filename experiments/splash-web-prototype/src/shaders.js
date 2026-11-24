const header = `#version 300 es
precision highp float;
`;

export const screenVertex = `${header}
out vec2 uv;
void main() {
  vec2 p = vec2((gl_VertexID << 1) & 2, gl_VertexID & 2);
  uv = vec2(p.x, 1.0 - p.y);
  gl_Position = vec4(p * 2.0 - 1.0, 0, 1);
}`;

export const atmosphereFragment = `${header}
in vec2 uv;
out vec4 outColor;
uniform vec2 size;
uniform float time, formed, wake, dawn;
uniform vec4 cores[4];

float hash(vec2 p) {
  return fract(52.9829189 * fract(dot(p, vec2(0.06711056, 0.00583715))));
}
float noise(vec2 p) {
  vec2 i = floor(p), f = fract(p);
  f = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash(i), hash(i+vec2(1,0)), f.x), mix(hash(i+vec2(0,1)), hash(i+1.0), f.x), f.y);
}
float fog(vec2 p) {
  return noise(p)*0.57 + noise(p*2.13+5.7)*0.26 + noise(p*4.23-2.3)*0.12 + noise(p*8.54)*0.05;
}
void main() {
  float aspect = size.x / size.y;
  vec2 p = (uv - 0.5) * vec2(aspect, 1.0);
  float field = fog(vec2(p.x*3.7-time*.054, p.y*7.0 + sin(p.x*4.0+time*.13)*.64));
  float m = fog(p*vec2(7.0,16.0) + vec2(time*.061, field*1.8));
  float level = p.y-.095 + .092*sin(p.x*3.6-time*.16) + .033*sin(p.x*8.4+time*.22);
  float shelf = exp(-5.4*abs(level)) * smoothstep(.24,.84,m);
  float haze = exp(-5.0*length(p*vec2(.60,1.0))) * field;
  float beams = pow(max(0.0, sin((p.x + p.y*.5)*14.0 + 2.9*field)), 12.0);
  float wakes = 0.0;
  for (int i=0; i<4; i++) {
    float lane = float(i);
    float y = .51+lane*.10-.12*(uv.x-.5)+sin(uv.x*6.7-time*.42-lane*.62)*(.079+lane*.022)
      +sin(uv.x*13.1+time*.32+lane*1.33)*.031;
    float distance = (uv.y-y)*38.0;
    float band = exp(-distance*distance);
    float glint = smoothstep(.27,.79,noise(vec2(uv.x*14.0-time*.7+lane*79.0,2.7+lane*8.2)));
    wakes += band * glint;
  }
  float sea = (shelf*.104 + haze*.087 + beams*shelf*.024 + wakes*.060*wake) * (1.0-formed*.50);

  float spin = 0.0;
  for (int i=0; i<4; ++i) {
    vec2 q = (uv-cores[i].xy) * vec2(aspect, 2.05);
    float a = atan(q.y,q.x), r = length(q);
    float arm = .5 + .5*cos((2.0+float(i%2))*a + 16.0*r + time*.61*cores[i].w + float(i)*2.1);
    float curl = pow(arm, 6.0) * exp(-r*6.5) * smoothstep(.028,.13,r);
    spin += (curl*.105 + exp(-23.0*r)*.035) * cores[i].z;
  }
  vec3 color = vec3(.008,.021,.015) + vec3(.21,.87,.51) * (sea+spin*(1.0-formed*.9));
  float vignette = clamp(1.12 - dot(p*vec2(.63,1.06),p*vec2(.63,1.06)), .18, 1.0);
  color *= vignette;
  color += (hash(gl_FragCoord.xy + mod(floor(time*24.0),127.0)*.47)-.5)*.021;
  // The light changes across the whole space as the print settles.
  color += vec3(.013,.08,.042) * (shelf+haze) * dawn * (1.0-dawn);
  float scatteredLight = (field-.5)*.61 * dawn * (1.0-dawn);
  float opacity = 1.0-clamp(dawn+scatteredLight,0.0,1.0);
  outColor = vec4(max(vec3(0),color)*opacity,opacity);
}`;

export const glyphVertex = `${header}
layout(location=0) in vec4 pose;
layout(location=1) in vec4 atlas;
layout(location=2) in vec4 tint;
layout(location=3) in vec4 optics;
uniform vec2 size;
out vec2 uv;
out vec2 localUV;
out vec4 ink;
out vec4 lens;
out vec2 screenUV;
void main() {
  vec2 corner = vec2(gl_VertexID & 1, gl_VertexID >> 1);
  vec2 p = (corner-.5)*pose.zw;
  float c = cos(optics.x), s = sin(optics.x);
  p = mat2(c,s,-s,c)*p;
  vec2 screen = (p + pose.xy)/size;
  gl_Position = vec4(screen*vec2(2,-2)+vec2(-1,1),0,1);
  uv = mix(atlas.xy,atlas.zw,corner);
  localUV = corner;
  screenUV = screen;
  ink = tint;
  lens = vec4(optics.y, (atlas.w-atlas.y)/35.0, optics.z, optics.w);
}`;

export const glyphFragment = `${header}
in vec2 uv;
in vec2 localUV;
in vec4 ink;
in vec4 lens;
in vec2 screenUV;
out vec4 outColor;
uniform sampler2D letters, artwork, support;
uniform vec4 pictureBounds;
uniform float daylight;
void main() {
  float glow = lens.x;
  float fill = texture(letters,uv).r;
  vec2 d = vec2(lens.y*.5, lens.y) * (1.2 + glow*1.8);
  float soft = (texture(letters,uv+vec2(d.x,0)).r + texture(letters,uv-vec2(d.x,0)).r
               + texture(letters,uv+vec2(0,d.y)).r + texture(letters,uv-vec2(0,d.y)).r) * .25;
  vec2 inner = min(localUV,1.0-localUV);
  float halo = mix(.16,.025,lens.z);
  float alpha = mix(fill,soft,glow*.72) + soft*halo;
  alpha = mix(alpha,pow(min(1.0,alpha),.40),lens.z);
  alpha *= smoothstep(0.0,.09,min(inner.x,inner.y));
  vec3 color = ink.rgb;
  float settled = smoothstep(.60,1.0,lens.z);
  if (settled > 0.0) {
    // The fixed glyph picks up detail from its exact position in the engraving.
    vec2 art = (screenUV-pictureBounds.xy)/pictureBounds.zw;
    vec3 source = texture(artwork,art).rgb;
    float value = smoothstep(.771,.925,dot(source,vec3(.45,.46,.09)));
    float localInk = smoothstep(.036,.139,texture(support,art).r);
    vec3 pigment = mix(vec3(.015,.052,.037),vec3(.77,.96,.85),value);
    float eye = min(length((art-vec2(.679,.274))/vec2(.022,.028)),length((art-vec2(.744,.234))/vec2(.020,.030)));
    float iris = (1.0-smoothstep(.42,1.0,eye)) * smoothstep(.056,.148,source.g-source.r);
    pigment += vec3(.08,.46,.26)*iris;
    pigment = mix(pigment,vec3(.62,.89,.75),lens.w * localInk * .46);
    // As the paper lights up, the luminous letters take on the color of ink.
    vec3 print = mix(source*vec3(.48,.71,.57),source,.32+.67*value);
    pigment = mix(pigment,print,smoothstep(.07,.58,daylight));
    color = mix(ink.rgb,pigment,settled*.98);
    alpha *= mix(1.0,.70+.30*localInk,settled);
  }
  alpha *= ink.a;
  outColor = vec4(color*alpha,alpha);
}`;
