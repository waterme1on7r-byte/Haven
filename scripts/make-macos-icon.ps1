param(
    [string]$Iconset = "assets/haven.iconset",
    [string]$Output = "assets/haven.icns"
)

$entries = @(
    @('icp4', 'icon_16x16.png'), @('icp5', 'icon_32x32.png'),
    @('icp6', 'icon_32x32@2x.png'), @('ic07', 'icon_128x128.png'),
    @('ic08', 'icon_256x256.png'), @('ic09', 'icon_512x512.png'),
    @('ic10', 'icon_512x512@2x.png')
)
function Be32([int]$n) {
    $b = [BitConverter]::GetBytes([uint32]$n)
    [Array]::Reverse($b)
    return $b
}
$parts = [System.Collections.Generic.List[byte[]]]::new()
$length = 8
foreach ($entry in $entries) {
    $png = [IO.File]::ReadAllBytes((Join-Path $Iconset $entry[1]))
    $type = [Text.Encoding]::ASCII.GetBytes($entry[0])
    $part = New-Object byte[] (8 + $png.Length)
    [Array]::Copy($type, 0, $part, 0, 4)
    [Array]::Copy((Be32 $part.Length), 0, $part, 4, 4)
    [Array]::Copy($png, 0, $part, 8, $png.Length)
    $parts.Add($part)
    $length += $part.Length
}
$out = New-Object byte[] $length
[Array]::Copy([Text.Encoding]::ASCII.GetBytes('icns'), 0, $out, 0, 4)
[Array]::Copy((Be32 $length), 0, $out, 4, 4)
$offset = 8
foreach ($part in $parts) {
    [Array]::Copy($part, 0, $out, $offset, $part.Length)
    $offset += $part.Length
}
[IO.File]::WriteAllBytes($Output, $out)
