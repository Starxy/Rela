param(
    [Parameter(Mandatory = $true)][string]$SourcePath,
    [Parameter(Mandatory = $true)][string]$ArchivePath
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem
Add-Type -AssemblyName System.IO.Compression
$sourceRoot = [IO.Path]::GetFullPath($SourcePath).TrimEnd([IO.Path]::DirectorySeparatorChar)
$prefix = $sourceRoot + [IO.Path]::DirectorySeparatorChar
$rootName = [IO.Path]::GetFileName($sourceRoot)
$stream = [IO.File]::Open($ArchivePath, 'CreateNew', 'ReadWrite', 'None')
try {
    $archive = [IO.Compression.ZipArchive]::new($stream, [IO.Compression.ZipArchiveMode]::Create, $true, [Text.Encoding]::UTF8)
    try {
        foreach ($file in Get-ChildItem -LiteralPath $sourceRoot -Recurse -File) {
            if (-not $file.FullName.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase) -or
                ($file.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
                throw 'Portable source contains an unsafe file path.'
            }
            # .NET Framework CreateFromDirectory emits backslashes on Windows.
            # ZIP entry paths must use forward slashes for portable ZIP compatibility.
            $entryName = $rootName + '/' + $file.FullName.Substring($prefix.Length).Replace('\', '/')
            [IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
                $archive, $file.FullName, $entryName, [IO.Compression.CompressionLevel]::Optimal
            ) | Out-Null
        }
    } finally { $archive.Dispose() }
    $stream.Flush($true)
} finally { $stream.Dispose() }
