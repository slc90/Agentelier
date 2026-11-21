$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing.Common
$width = [int]$env:AGENTELIER_CAPTURE_WIDTH
$height = [int]$env:AGENTELIER_CAPTURE_HEIGHT
$outWidth = [int]$env:AGENTELIER_CAPTURE_OUT_WIDTH
$outHeight = [int]$env:AGENTELIER_CAPTURE_OUT_HEIGHT
$format = [System.Drawing.Imaging.PixelFormat]::Format24bppRgb
$source = [System.Drawing.Bitmap]::new($width, $height, $format)
try {
    $graphics = [System.Drawing.Graphics]::FromImage($source)
    try {
        $graphics.CopyFromScreen(
            [int]$env:AGENTELIER_CAPTURE_X, [int]$env:AGENTELIER_CAPTURE_Y,
            0, 0, $source.Size
        )
    } finally {
        $graphics.Dispose()
    }
    $frame = $source
    try {
        if ($outWidth -ne $width -or $outHeight -ne $height) {
            $frame = [System.Drawing.Bitmap]::new($outWidth, $outHeight, $format)
            $graphics = [System.Drawing.Graphics]::FromImage($frame)
            try {
                $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
                $graphics.DrawImage($source, 0, 0, $outWidth, $outHeight)
            } finally {
                $graphics.Dispose()
            }
        }
        $encoded = [System.IO.MemoryStream]::new()
        try {
            $frame.Save($encoded, [System.Drawing.Imaging.ImageFormat]::Png)
            $limit = [int]$env:AGENTELIER_CAPTURE_MAX_BYTES
            if ($encoded.Length -gt $limit) {
                $encoded.SetLength(0)
                $encoded.Position = 0
                $frame.Save($encoded, [System.Drawing.Imaging.ImageFormat]::Jpeg)
            }
            if ($encoded.Length -gt $limit) {
                throw 'Encoded capture exceeds the image payload limit'
            }
            $encoded.Position = 0
            $encoded.CopyTo([Console]::OpenStandardOutput())
        } finally {
            $encoded.Dispose()
        }
    } finally {
        if ($frame -ne $source) {
            $frame.Dispose()
        }
    }
} finally {
    $source.Dispose()
}
