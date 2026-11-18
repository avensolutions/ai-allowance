$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$root = Split-Path $PSScriptRoot -Parent
$output = Join-Path $root 'assets\installer'
New-Item -ItemType Directory -Force $output | Out-Null
$star = [System.Drawing.Image]::FromFile((Join-Path $root 'assets\source\favicon-96x96.png'))

function Save-Artwork([string]$Name, [int]$Width, [int]$Height, [array]$Stars) {
    $bitmap = [System.Drawing.Bitmap]::new($Width, $Height, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.Clear([System.Drawing.Color]::White)
        $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        foreach ($placement in $Stars) {
            $graphics.DrawImage($star, [System.Drawing.Rectangle]::new(
                $placement.X, $placement.Y, $placement.Size, $placement.Size
            ))
        }
        $bitmap.Save((Join-Path $output $Name), [System.Drawing.Imaging.ImageFormat]::Bmp)
    }
    finally {
        $graphics.Dispose()
        $bitmap.Dispose()
    }
}

try {
    Save-Artwork 'sidebar.bmp' 164 314 @(
        @{ X = 18; Y = 36; Size = 96 },
        @{ X = 62; Y = 192; Size = 84 }
    )
    Save-Artwork 'header.bmp' 150 57 @(
        @{ X = 96; Y = 7; Size = 42 }
    )
}
finally {
    $star.Dispose()
}
