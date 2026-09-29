# Exercise the cleanup guards with fixtures; never touch a Windows service.
$ErrorActionPreference = 'Stop'
$source = Join-Path $PSScriptRoot '../packaging/portable/Remove-Network-Service.ps1'
$ast = [Management.Automation.Language.Parser]::ParseFile($source, [ref]$null, [ref]$null)
$functions = $ast.FindAll({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] }, $false)
foreach ($definition in $functions) {
    . ([scriptblock]::Create($definition.Extent.Text))
}

$fixture = Join-Path ([IO.Path]::GetTempPath()) ('rela-cleanup-' + [guid]::NewGuid().ToString('N'))
$programData = $fixture
$root = Join-Path $fixture 'Rela'
$serviceName = 'RelaEasyTier'
$expectedBinaries = @((Join-Path $root 'engine\easytier-core.exe'), (Join-Path $root 'engine-v2\easytier-core.exe'))
[IO.Directory]::CreateDirectory((Join-Path $root 'engine')) | Out-Null
[IO.File]::WriteAllText((Join-Path $root 'engine\probe.txt'), 'fixture')

function Expect-Rejected([scriptblock]$Action, [string]$Message) {
    $rejected = $false
    try { & $Action | Out-Null } catch { $rejected = $true }
    if (-not $rejected) { throw $Message }
}

try {
    Assert-SafeDirectory
    function Get-CimInstance { [pscustomobject]@{ PathName = '"C:\Other\easytier-core.exe" --service' } }
    Expect-Rejected { Get-RelaService } 'A foreign service path was accepted.'
    foreach ($expected in $expectedBinaries) {
        foreach ($script:testBinary in @($expected, ('\\?\' + $expected))) {
            function Get-CimInstance { [pscustomobject]@{ PathName = '"' + $script:testBinary + '" --service' } }
            if (-not (Get-RelaService)) { throw 'Owned service not recognized.' }
        }
    }
    # Substitute only the directory's metadata to exercise the reparse guard.
    function Get-Item {
        param([string]$LiteralPath, [switch]$Force)
        $item = Microsoft.PowerShell.Management\Get-Item -LiteralPath $LiteralPath -Force
        [pscustomobject]@{
            PSIsContainer = $item.PSIsContainer
            FullName = $item.FullName
            Parent = $item.Parent
            Attributes = [IO.FileAttributes]::ReparsePoint
        }
    }
    Expect-Rejected { Assert-SafeDirectory } 'A reparse point was accepted.'
    Write-Host 'Portable cleanup guards passed: own service accepted; foreign service and reparse point refused.'
} finally {
    # Remove only the exact files and empty folders this test created.
    [IO.File]::Delete((Join-Path $root 'engine\probe.txt'))
    [IO.Directory]::Delete((Join-Path $root 'engine'))
    [IO.Directory]::Delete($root)
    [IO.Directory]::Delete($fixture)
}
