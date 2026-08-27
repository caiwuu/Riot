import tinycolor from 'tinycolor2';
export const indexedColors = [
    'FF000000', 'FFFFFFFF', 'FFFF0000', 'FF00FF00', 'FF0000FF', 'FFFFFF00', 'FFFF00FF', 'FF00FFFF', // 0-7
    'FF000000', 'FFFFFFFF', 'FFFF0000', 'FF00FF00', 'FF0000FF', 'FFFFFF00', 'FFFF00FF', 'FF00FFFF', // 8-15
    'FF800000', 'FF008000', 'FF000080', 'FF808000', 'FF800080', 'FF008080', 'FFC0C0C0', 'FF808080', // 16-23
    'FF9999FF', 'FF993366', 'FFFFFFCC', 'FFCCFFFF', 'FF660066', 'FFFF8080', 'FF0066CC', 'FFCCCCFF', // 24-31
    'FF000080', 'FFFF00FF', 'FFFFFF00', 'FF00FFFF', 'FF800080', 'FF800000', 'FF008080', 'FF0000FF', // 32-39
    'FF00CCFF', 'FFCCFFFF', 'FFCCFFCC', 'FFFFFF99', 'FF99CCFF', 'FFFF99CC', 'FFCC99FF', 'FFFFCC99', // 40-47
    'FF3366FF', 'FF33CCCC', 'FF99CC00', 'FFFFCC00', 'FFFF9900', 'FFFF6600', 'FF666699', 'FF969696', // 48-55
    'FF003366', 'FF339966', 'FF003300', 'FF333300', 'FF993300', 'FF993366', 'FF333399', 'FF333333' // 56-63
];
const HLSMAX = 240;
const RGBMAX = 0xFF;
// Excel 主题色的 tint 计算使用微软 HLS 取值范围，和浏览器 HSL 不是同一套刻度。
function rgb2hls(r, g, b) {
    const maxc = Math.max(r, g, b);
    const minc = Math.min(r, g, b);
    const sumc = maxc + minc;
    const rangec = maxc - minc;
    const l = sumc / 2.0;
    let h, s;
    if (minc == maxc) {
        return [0.0, l, 0.0];
    }
    if (l <= 0.5) {
        s = rangec / sumc;
    }
    else {
        s = rangec / (2.0 - sumc);
    }
    const rc = (maxc - r) / rangec;
    const gc = (maxc - g) / rangec;
    const bc = (maxc - b) / rangec;
    if (r == maxc) {
        h = bc - gc;
    }
    else if (g === maxc) {
        h = 2.0 + rc - bc;
    }
    else {
        h = 4.0 + gc - rc;
    }
    h = (h / 6.0) % 1.0;
    return [h, l, s];
}
function rgb2MsHls(hex) {
    if (hex.length > 6) {
        hex = hex.substring(2);
    }
    const red = parseInt(hex.slice(0, 2), 16) / RGBMAX;
    const green = parseInt(hex.slice(2, 4), 16) / RGBMAX;
    const blue = parseInt(hex.slice(4, 6), 16) / RGBMAX;
    const [h, l, s] = rgb2hls(red, green, blue);
    return [Math.round(h * HLSMAX), Math.round(l * HLSMAX), Math.round(s * HLSMAX)];
}
function msHls2Rgb(hue, lightness, saturation) {
    const color = tinycolor({ h: hue / HLSMAX * 360, s: saturation / HLSMAX, l: lightness / HLSMAX });
    return color.toHex().toUpperCase();
}
function tintLuminance(tint, lum) {
    if (tint <= 0) {
        return Math.round(lum * (1.0 + tint));
    }
    return Math.round(lum * (1.0 - tint) + (HLSMAX - HLSMAX * (1.0 - tint)));
}
// 根据主题原始色和 tint 还原 Excel 实际显示色。
export function getTintColor(hex, tint) {
    if (!hex) {
        return hex;
    }
    const [h, l, s] = rgb2MsHls(hex);
    return `FF${msHls2Rgb(h, tintLuminance(tint, l), s)}`;
}
