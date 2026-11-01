# Asks Windows itself for a project's thumbnail, the way Explorer does, and
# checks it's the project's picture (red).
param([string]$File)
$ErrorActionPreference = "Stop"
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class Thumb {
    [StructLayout(LayoutKind.Sequential)] public struct SIZE { public int cx; public int cy; }
    [ComImport, Guid("bcc18b79-ba16-442f-80c4-8a59c30c463b"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IShellItemImageFactory { [PreserveSig] int GetImage(SIZE size, int flags, out IntPtr phbm); }
    [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
    static extern void SHCreateItemFromParsingName(string path, IntPtr pbc, ref Guid riid, [MarshalAs(UnmanagedType.Interface)] out IShellItemImageFactory ppv);
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
        using (var b = System.Drawing.Image.FromHbitmap(h)) {
            var c = b.GetPixel(b.Width / 2, b.Height / 2);
            DeleteObject(h);
            return b.Width + "x" + b.Height + " " + c.R + "," + c.G + "," + c.B;
        }
    }
}
"@
$r = [Thumb]::Get((Resolve-Path $File).Path)
Write-Host "::notice::Explorer thumbnail: $r"
if ($r -notmatch "^\d+x\d+ (\d+),(\d+),(\d+)$" -or [int]$Matches[1] -lt 150 -or [int]$Matches[3] -gt 100) { Write-Host "::error::wrong thumbnail: $r"; exit 1 }
