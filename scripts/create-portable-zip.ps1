param(
    [Parameter(Mandatory = $true)][string]$SourcePath,
    [Parameter(Mandatory = $true)][string]$ArchivePath
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::CreateFromDirectory(
    $SourcePath,
    $ArchivePath,
    [System.IO.Compression.CompressionLevel]::Optimal,
    $true,
    [System.Text.Encoding]::UTF8
)
