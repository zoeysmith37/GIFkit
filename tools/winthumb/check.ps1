# Asks Windows itself for a project's thumbnail, the way Explorer does, and
# checks it's the project's picture (red).
param([string]$File)
$ErrorActionPreference = "Stop"
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class Thumb {
    [StructLayout(LayoutKind.Sequential)] public struct SIZE { public int cx; public int cy; }
    [StructLayout(LayoutKind.Sequential)] struct BITMAP { public int bmType; public int bmWidth; public int bmHeight; public int bmWidthBytes; public ushort bmPlanes; public ushort bmBitsPixel; public IntPtr bmBits; }
    [StructLayout(LayoutKind.Sequential)] struct BITMAPINFOHEADER { public int biSize; public int biWidth; public int biHeight; public short biPlanes; public short biBitCount; public int biCompression; public int biSizeImage; public int biXPelsPerMeter; public int biYPelsPerMeter; public int biClrUsed; public int biClrImportant; }
    [ComImport, Guid("bcc18b79-ba16-442f-80c4-8a59c30c463b"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IShellItemImageFactory { [PreserveSig] int GetImage(SIZE size, int flags, out IntPtr phbm); }
    [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
    static extern void SHCreateItemFromParsingName(string path, IntPtr pbc, ref Guid riid, [MarshalAs(UnmanagedType.Interface)] out IShellItemImageFactory ppv);
    [DllImport("gdi32.dll")] static extern int GetObject(IntPtr h, int size, ref BITMAP bm);
    [DllImport("gdi32.dll")] static extern int GetDIBits(IntPtr hdc, IntPtr hbm, uint start, uint lines, byte[] bits, ref BITMAPINFOHEADER bmi, uint usage);
    [DllImport("user32.dll")] static extern IntPtr GetDC(IntPtr w);
    [DllImport("user32.dll")] static extern int ReleaseDC(IntPtr w, IntPtr dc);
    [DllImport("gdi32.dll")] static extern bool DeleteObject(IntPtr h);
    public static string Get(string path) {
        Guid iid = new Guid("bcc18b79-ba16-442f-80c4-8a59c30c463b");
        IShellItemImageFactory f;
        SHCreateItemFromParsingName(path, IntPtr.Zero, ref iid, out f);
        IntPtr h;
        SIZE s; s.cx = 96; s.cy = 96;
        // 0x8: a thumbnail only, never the file type's icon
        int hr = f.GetImage(s, 0x8, out h);
        if (hr != 0) return "failed 0x" + hr.ToString("x8");
        BITMAP bm = new BITMAP();
        GetObject(h, Marshal.SizeOf(typeof(BITMAP)), ref bm);
        int w = bm.bmWidth, ht = Math.Abs(bm.bmHeight);
        BITMAPINFOHEADER bi = new BITMAPINFOHEADER();
        bi.biSize = Marshal.SizeOf(typeof(BITMAPINFOHEADER));
        bi.biWidth = w; bi.biHeight = -ht; bi.biPlanes = 1; bi.biBitCount = 32;
        byte[] px = new byte[w * ht * 4];
        IntPtr dc = GetDC(IntPtr.Zero);
        GetDIBits(dc, h, 0, (uint)ht, px, ref bi, 0);
        ReleaseDC(IntPtr.Zero, dc);
        DeleteObject(h);
        int i = ((ht / 2) * w + w / 2) * 4;
        return w + "x" + ht + " " + px[i + 2] + "," + px[i + 1] + "," + px[i];
    }
}
"@
$r = [Thumb]::Get((Resolve-Path $File).Path)
Write-Host "::notice::Explorer thumbnail: $r"
if ($r -notmatch "^\d+x\d+ (\d+),(\d+),(\d+)$" -or [int]$Matches[1] -lt 150 -or [int]$Matches[3] -gt 100) { Write-Host "::error::wrong thumbnail: $r"; exit 1 }
