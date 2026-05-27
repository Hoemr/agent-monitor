// Generate multi-resolution pixel-art ICO for Agent Monitor
const fs = require('fs');
const zlib = require('zlib');

// Pixel art: "AM" monogram glyph at 16x16 (hand-drawn)
// Colors: 0=bg(#0a0a0f), 1=border(#1c1c30), 2=green(#00ff88), 3=amber(#ffaa00)
const art16 = [
  0,0,0,1,1,1,1,1,1,1,1,1,0,0,0,0,
  0,0,1,1,2,2,2,2,2,2,1,1,1,0,0,0,
  0,1,1,0,0,0,0,0,0,0,0,0,1,1,0,0,
  1,1,0,0,2,2,0,0,0,0,0,0,0,1,1,0,
  1,0,0,2,2,2,2,2,2,2,0,0,0,0,1,0,
  1,0,2,2,0,0,0,2,2,0,0,0,0,0,1,0,
  1,0,2,2,2,2,2,2,2,2,2,0,3,0,1,0,
  1,0,2,2,0,0,0,0,0,2,2,0,3,0,1,0,
  1,0,2,2,0,0,0,0,2,2,0,3,0,0,1,0,
  1,0,0,2,2,2,2,2,2,0,3,0,0,0,1,0,
  0,1,0,0,0,0,0,0,3,3,0,0,0,1,0,0,
  0,0,1,0,0,0,0,3,3,0,0,0,1,0,0,0,
  0,0,0,1,0,0,0,0,0,0,0,1,0,0,0,0,
  0,0,0,0,1,0,0,0,0,0,1,0,0,0,0,0,
  0,0,0,0,0,1,1,1,1,1,0,0,0,0,0,0,
  0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
];

// 32x32: detailed version (same as before, scaled up)
const art32 = [
  0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
  0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
  0,0,0,0,0,0,0,0,0,0,1,1,1,1,1,1,1,1,1,1,1,1,1,0,0,0,0,0,0,0,0,0,
  0,0,0,0,0,0,0,0,0,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,0,0,0,0,0,0,0,0,
  0,0,0,0,0,0,0,0,1,1,0,0,0,0,0,0,0,0,0,0,0,0,1,1,1,0,0,0,0,0,0,0,
  0,0,0,0,0,0,0,1,1,0,0,0,0,0,2,2,0,0,0,0,0,0,0,1,1,0,0,0,0,0,0,0,
  0,0,0,0,0,0,1,1,0,0,0,0,2,2,2,2,2,0,0,0,0,0,0,0,1,1,0,0,0,0,0,0,
  0,0,0,0,0,1,1,0,0,0,0,2,2,0,2,2,2,0,0,0,0,0,0,0,1,1,0,0,0,0,0,0,
  0,0,0,0,0,1,1,0,0,0,0,2,2,0,0,2,2,0,0,0,0,0,0,0,1,1,0,0,0,0,0,0,
  0,0,0,0,1,1,0,0,0,0,2,2,0,0,0,2,2,0,0,0,0,0,0,0,0,1,1,0,0,0,0,0,
  0,0,0,0,1,1,0,0,0,2,2,2,2,2,2,2,2,2,0,0,0,0,0,0,0,1,1,0,0,0,0,0,
  0,0,0,0,1,1,0,0,0,2,2,2,2,2,2,2,2,2,0,0,0,0,0,0,0,1,1,0,0,0,0,0,
  0,0,0,0,1,1,0,0,2,2,0,0,0,0,0,0,2,2,0,0,0,0,0,0,0,1,1,0,0,0,0,0,
  0,0,0,0,1,1,0,0,2,2,0,0,0,0,0,0,2,2,0,0,0,0,0,0,0,1,1,0,0,0,0,0,
  0,0,0,0,1,1,0,0,2,2,0,0,0,0,0,0,2,2,0,0,0,0,0,0,0,1,1,0,0,0,0,0,
  0,0,0,0,1,1,0,0,2,2,2,2,2,2,2,2,2,2,0,0,0,0,0,0,0,1,1,0,0,0,0,0,
  0,0,0,0,1,1,0,0,2,2,2,2,2,2,2,2,2,2,0,0,0,0,3,3,0,1,1,0,0,0,0,0,
  0,0,0,0,1,1,0,0,2,2,0,0,0,0,0,0,2,2,0,0,0,0,3,3,0,1,1,0,0,0,0,0,
  0,0,0,0,1,1,0,0,2,2,0,0,0,0,0,0,2,2,0,0,0,3,3,0,0,1,1,0,0,0,0,0,
  0,0,0,0,0,1,1,0,0,2,2,0,0,0,0,2,2,0,0,0,3,3,0,0,1,1,0,0,0,0,0,0,
  0,0,0,0,0,1,1,0,0,0,2,2,2,2,2,2,2,0,0,3,3,0,0,0,1,1,0,0,0,0,0,0,
  0,0,0,0,0,0,1,1,0,0,0,0,0,0,0,0,0,0,3,3,0,0,0,0,1,1,0,0,0,0,0,0,
  0,0,0,0,0,0,0,1,1,0,0,0,0,0,0,0,0,3,3,0,0,0,0,1,1,0,0,0,0,0,0,0,
  0,0,0,0,0,0,0,0,1,1,0,0,0,0,0,0,0,0,0,0,0,0,1,1,1,0,0,0,0,0,0,0,
  0,0,0,0,0,0,0,0,0,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,0,0,0,0,0,0,0,0,
  0,0,0,0,0,0,0,0,0,0,1,1,1,1,1,1,1,1,1,1,1,1,1,0,0,0,0,0,0,0,0,0,
  0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
  0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
  0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
  0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
  0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
  0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
];

const colors = [
  [0x0a,0x0a,0x0f,255], [0x1c,0x1c,0x30,255],
  [0x00,0xff,0x88,255], [0xff,0xaa,0x00,255],
];

function crc32(buf) {
  const t = new Int32Array(256);
  for (let n=0; n<256; n++) { let c=n; for (let k=0;k<8;k++) c=(c&1)?(0xEDB88320^(c>>>1)):(c>>>1); t[n]=c; }
  let c=0xFFFFFFFF;
  for (let i=0;i<buf.length;i++) c=t[(c^buf[i])&0xFF]^(c>>>8);
  return (c^0xFFFFFFFF)>>>0;
}

function buildBmp(pixels, w, h) {
  const row = w*4;
  const data = Buffer.alloc(row*h);
  for (let y=0; y<h; y++) {
    for (let x=0; x<w; x++) {
      const c = colors[pixels[(h-1-y)*w+x]];
      const o = y*row + x*4;
      data[o]=c[2]; data[o+1]=c[1]; data[o+2]=c[0]; data[o+3]=c[3];
    }
  }
  const maskR = Math.ceil(w/8)*4;
  const mask = Buffer.alloc(maskR*h);
  for (let y=0; y<h; y++) {
    for (let x=0; x<w; x++) {
      if (pixels[(h-1-y)*w+x]===0) {
        const o = y*maskR + Math.floor(x/8);
        mask[o] |= (1 << (7-(x%8)));
      }
    }
  }
  // BITMAPINFOHEADER + XOR + AND
  const bih = Buffer.alloc(40);
  bih.writeUInt32LE(40,0); bih.writeInt32LE(w,4); bih.writeInt32LE(h*2,8);
  bih.writeUInt16LE(1,12); bih.writeUInt16LE(32,14);
  bih.writeUInt32LE(0,16); bih.writeUInt32LE(data.length+mask.length,20);
  return Buffer.concat([bih, data, mask]);
}

function makeICO() {
  // Generate sizes: 16, 32, 48 (scaled from 16), 256 (scaled from 32)
  const sizes = [
    { pixels: art16, w: 16, h: 16 },
    { pixels: art32, w: 32, h: 32 },
  ];

  // 48x48: scale 16x16 by 3x
  const art48 = [];
  for (let y=0; y<16; y++)
    for (let yy=0; yy<3; yy++)
      for (let x=0; x<16; x++)
        for (let xx=0; xx<3; xx++)
          art48.push(art16[y*16+x]);
  sizes.push({ pixels: art48, w: 48, h: 48 });

  // 256x256: scale 32x32 by 8x
  const art256 = [];
  for (let y=0; y<32; y++)
    for (let yy=0; yy<8; yy++)
      for (let x=0; x<32; x++)
        for (let xx=0; xx<8; xx++)
          art256.push(art32[y*32+x]);
  sizes.push({ pixels: art256, w: 256, h: 256 });

  let header = Buffer.alloc(6);
  header.writeUInt16LE(0,0); header.writeUInt16LE(1,2); header.writeUInt16LE(sizes.length,4);

  let entries = Buffer.alloc(sizes.length*16);
  let imgData = Buffer.alloc(0);
  let offset = 6 + sizes.length*16;

  for (let i=0; i<sizes.length; i++) {
    const {pixels, w, h} = sizes[i];
    const bmp = buildBmp(pixels, w, h);
    imgData = Buffer.concat([imgData, bmp]);

    const e = i*16;
    entries.writeUInt8(w>=256?0:w, e);
    entries.writeUInt8(h>=256?0:h, e+1);
    entries.writeUInt8(0, e+2); entries.writeUInt8(0, e+3);
    entries.writeUInt16LE(1, e+4); entries.writeUInt16LE(32, e+6);
    entries.writeUInt32LE(bmp.length, e+8);
    entries.writeUInt32LE(offset, e+12);
    offset += bmp.length;
  }

  fs.writeFileSync('src-tauri/icons/icon.ico', Buffer.concat([header, entries, imgData]));
  console.log('ICO generated: ' + sizes.length + ' sizes (' + (6+sizes.length*16+imgData.length) + ' bytes)');
}

// Generate PNG for tray (32x32)
function makePNG(w, h, pixels) {
  const raw = Buffer.alloc((w*4+1)*h);
  for (let y=0; y<h; y++) {
    raw[y*(w*4+1)]=0; // filter
    for (let x=0; x<w; x++) {
      const c = colors[pixels[y*w+x]];
      const o = y*(w*4+1)+1+x*4;
      raw[o]=c[0]; raw[o+1]=c[1]; raw[o+2]=c[2]; raw[o+3]=c[3];
    }
  }
  const def = zlib.deflateSync(raw);
  const sig = Buffer.from([137,80,78,71,13,10,26,10]);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w,0); ihdr.writeUInt32BE(h,4);
  ihdr[8]=8; ihdr[9]=6; ihdr[10]=0; ihdr[11]=0; ihdr[12]=0;

  function chunk(t,d) {
    const l=Buffer.alloc(4); l.writeUInt32BE(d.length);
    const tb=Buffer.from(t); const cr=Buffer.alloc(4);
    cr.writeUInt32BE(crc32(Buffer.concat([tb,d])));
    return Buffer.concat([l,tb,d,cr]);
  }
  return Buffer.concat([sig, chunk('IHDR',ihdr), chunk('IDAT',def), chunk('IEND',Buffer.alloc(0))]);
}

makeICO();
fs.writeFileSync('src-tauri/icons/icon.png', makePNG(32, 32, art32));
console.log('PNG tray icon: ' + makePNG(32,32,art32).length + ' bytes');
