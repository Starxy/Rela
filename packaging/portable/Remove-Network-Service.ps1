param([switch]$CheckOnly)
$ErrorActionPreference = 'Stop'
$serviceName = 'RelaEasyTier'
$programData = [Environment]::GetFolderPath('CommonApplicationData')
$root = [IO.Path]::GetFullPath((Join-Path $programData 'Rela'))
$expectedBinaries = @(
    (Join-Path $root 'engine\easytier-core.exe'),
    (Join-Path $root 'engine-v2\easytier-core.exe')
)

function Assert-SafeDirectory {
    if (-not (Test-Path -LiteralPath $root)) { return }
    $directory = Get-Item -LiteralPath $root -Force
    if (-not $directory.PSIsContainer -or
        $directory.FullName -ine $root -or
        $directory.Parent.FullName -ine $programData) {
        throw 'Unexpected service data path. No data was removed.'
    }
    $pending = New-Object 'System.Collections.Generic.Stack[string]'
    $pending.Push($root)
    while ($pending.Count -gt 0) {
        $entry = Get-Item -LiteralPath $pending.Pop() -Force
        if ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw 'Service data contains a reparse point. No data was removed.'
        }
        if ($entry.PSIsContainer) {
            foreach ($child in Get-ChildItem -LiteralPath $entry.FullName -Force) {
                $pending.Push($child.FullName)
            }
        }
    }
}

function Get-RelaService {
    $service = Get-CimInstance Win32_Service -Filter "Name='$serviceName'"
    if ($service) {
        $command = $service.PathName
        $owned = $false
        foreach ($expectedBinary in $expectedBinaries) {
            foreach ($binary in @($expectedBinary, ('\\?\' + $expectedBinary))) {
                if ($command.StartsWith(('"' + $binary + '" '), [StringComparison]::OrdinalIgnoreCase) -or
                    $command.StartsWith(($binary + ' '), [StringComparison]::OrdinalIgnoreCase)) {
                    $owned = $true
                }
            }
        }
        if (-not $owned) {
            throw 'Service path does not belong to Rela. Nothing was changed.'
        }
    }
    return $service
}

try {
    if (-not $CheckOnly) {
        $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
        $principal = New-Object Security.Principal.WindowsPrincipal($identity)
        if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
            throw 'Right-click Remove-Network-Service.cmd and choose Run as administrator.'
        }
    }
    $service = Get-RelaService
    Assert-SafeDirectory
    Write-Host "Service: $serviceName; registered: $([bool]$service)"
    Write-Host "Service data: $root; exists: $(Test-Path -LiteralPath $root)"
    if ($CheckOnly) { exit 0 }
    Write-Host 'This disconnects Rela, removes its service and service data, including the saved service configuration.'
    Write-Host 'The installed edition uses the same service. Close every Rela window before continuing.'
    if ((Read-Host 'Type REMOVE to continue') -cne 'REMOVE') {
        Write-Host 'Cancelled. Nothing was changed.'
        exit 0
    }

    $controlLock = $null
    try {
        if (Test-Path -LiteralPath $root) {
            $controlLock = [IO.File]::Open((Join-Path $root 'control.lock'), 'OpenOrCreate', 'ReadWrite', 'None')
        }
        $service = Get-RelaService
        if ($service) {
            $controller = Get-Service -Name $serviceName
            if ($controller.Status -ne 'Stopped') {
                Stop-Service -Name $serviceName -ErrorAction Stop
                $controller.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(30))
            }
            $controller.Dispose()
            & "$env:SystemRoot\System32\sc.exe" delete $serviceName
            if ($LASTEXITCODE -ne 0) { throw 'Unable to delete the service. Service data was kept.' }
            $deadline = [DateTime]::UtcNow.AddSeconds(30)
            while (Get-CimInstance Win32_Service -Filter "Name='$serviceName'") {
                if ([DateTime]::UtcNow -gt $deadline) {
                    throw 'Service deletion is pending. Restart Windows and run this tool again. Service data was kept.'
                }
                Start-Sleep -Milliseconds 250
            }
        }
    } finally {
        if ($controlLock) { $controlLock.Dispose() }
    }
    Assert-SafeDirectory
    if (Test-Path -LiteralPath $root) {
        Remove-Item -LiteralPath $root -Recurse -Force
    }
    Write-Host 'Done. The Rela network service and service data have been removed.'
    Write-Host 'You can now delete the Portable folder. Shared drivers and WebView2 were left in place.'
} catch {
    Write-Error $_
    exit 1
}
