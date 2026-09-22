[CmdletBinding()]
param(
    [string]$Version,
    [string]$Archive,
    [string]$Sha256,
    [string]$Prefix,
    [string]$PluginRoot
)

$ErrorActionPreference = "Stop"
$Repository = "https://github.com/ilanbm/kpopper"

function Fail([string]$Message) { throw "kpopper installer: $Message" }
function Assert-Version([string]$Value) {
    if ([string]::IsNullOrWhiteSpace($Value) -or $Value -notmatch '^[A-Za-z0-9._-]+$' -or $Value -in @('.', '..')) {
        Fail "invalid version: $Value"
    }
}
function Get-TreeInventory([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) { Fail "managed resource tree is missing: $Path" }
    $Resolved = (Resolve-Path -LiteralPath $Path).Path.TrimEnd([char[]]"\/")
    $Rows = [Collections.Generic.List[string]]::new()
    foreach ($Item in @(Get-ChildItem -LiteralPath $Resolved -Force -Recurse | Sort-Object FullName)) {
        if (($Item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            Fail "managed resource tree contains a link: $($Item.FullName)"
        }
        $Relative = $Item.FullName.Substring($Resolved.Length).TrimStart([char[]]"\/").Replace('\', '/')
        if ($Item.PSIsContainer) {
            $Rows.Add("D|$Relative")
        } elseif (Test-Path -LiteralPath $Item.FullName -PathType Leaf) {
            $Rows.Add("F|$Relative|$((Get-FileHash -LiteralPath $Item.FullName -Algorithm SHA256).Hash)")
        } else {
            Fail "managed resource tree contains a special file: $($Item.FullName)"
        }
    }
    return $Rows.ToArray()
}

if ($Prefix -and $PluginRoot) { Fail "-Prefix and -PluginRoot cannot be combined" }
if (-not [Environment]::Is64BitOperatingSystem -or $env:PROCESSOR_ARCHITECTURE -notin @('AMD64', 'x86')) {
    Fail "unsupported native platform: Windows $env:PROCESSOR_ARCHITECTURE"
}
$Target = "windows-x86_64"
$Work = Join-Path ([IO.Path]::GetTempPath()) ("kpopper-install-" + [Guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($Work) | Out-Null

try {
    if ($Archive) {
        if (-not $Version) { Fail "-Version is required with -Archive" }
        if (-not $Sha256) { Fail "-Sha256 is required with -Archive" }
        if (-not (Test-Path -LiteralPath $Archive -PathType Leaf)) { Fail "archive does not exist: $Archive" }
        $Archive = (Resolve-Path -LiteralPath $Archive).Path
    } else {
        if ($Sha256) { Fail "-Sha256 is only valid with -Archive" }
        if (-not $Version) {
            $Latest = Invoke-RestMethod -Uri "https://api.github.com/repos/ilanbm/kpopper/releases/latest" -MaximumRedirection 5
            $Version = [string]$Latest.tag_name
            if ($Version.StartsWith('v')) { $Version = $Version.Substring(1) }
        }
        Assert-Version $Version
        $Name = "kpopper-$Version-$Target.zip"
        $Archive = Join-Path $Work $Name
        Invoke-WebRequest -Uri "$Repository/releases/download/v$Version/$Name" -OutFile $Archive -MaximumRedirection 5
        $Sums = Join-Path $Work "SHA256SUMS"
        Invoke-WebRequest -Uri "$Repository/releases/download/v$Version/SHA256SUMS" -OutFile $Sums -MaximumRedirection 5
        $Pattern = '^([0-9A-Fa-f]{64})\s+\*?' + [regex]::Escape($Name) + '$'
        $Match = Get-Content -LiteralPath $Sums | Where-Object { $_ -match $Pattern } | Select-Object -First 1
        if (-not $Match) { Fail "archive is absent from SHA256SUMS" }
        $null = $Match -match $Pattern
        $Sha256 = $Matches[1]
    }
    Assert-Version $Version
    if ($Sha256 -notmatch '^[0-9A-Fa-f]{64}$') { Fail "invalid SHA256" }
    $Actual = (Get-FileHash -LiteralPath $Archive -Algorithm SHA256).Hash
    if ($Actual -ne $Sha256) { Fail "archive SHA256 does not match" }

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $Top = "kpopper-$Version-$Target"
    $Zip = [IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        $Names = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        foreach ($Entry in $Zip.Entries) {
            $Name = $Entry.FullName
            if ([string]::IsNullOrEmpty($Name) -or $Name.StartsWith('/') -or $Name.Contains('\') -or
                $Name -notlike "$Top*" -or ($Name -ne $Top -and -not $Name.StartsWith("$Top/"))) {
                Fail "unsafe or unexpected archive path: $Name"
            }
            foreach ($Part in $Name.Split('/')) {
                if ($Part -in @('.', '..') -or $Part -match '[<>:"|?*]' -or $Part -match '[ .]$' -or
                    $Part -match '[\x00-\x1F]') {
                    Fail "unsafe archive path: $Name"
                }
            }
            $UnixType = (($Entry.ExternalAttributes -shr 16) -band 0xF000)
            if ($UnixType -notin @(0, 0x4000, 0x8000)) { Fail "archive contains a link or special file: $Name" }
            $null = $Names.Add($Name)
        }
        foreach ($Required in @("$Top/manifest.json", "$Top/bin/kpop.exe", "$Top/bin/kpopper.exe",
                                "$Top/bin/resources/reasoning/", "$Top/bin/resources/ordinary/")) {
            if (-not $Names.Contains($Required)) { Fail "required archive member is missing: $Required" }
        }
        $Unpacked = Join-Path $Work "unpacked"
        [IO.Directory]::CreateDirectory($Unpacked) | Out-Null
        [IO.Compression.ZipFile]::ExtractToDirectory($Archive, $Unpacked)
    } finally {
        if ($Zip) { $Zip.Dispose() }
    }
    $Source = Join-Path $Unpacked "$Top/bin"
    $PackageRoot = Join-Path $Unpacked $Top

    if ($PluginRoot) {
        $Destination = Join-Path $PluginRoot "scripts/runtime/$Target"
        $Marker = Join-Path $Destination ".kpopper-managed"
        if ((Test-Path -LiteralPath $Destination) -and -not (Test-Path -LiteralPath $Marker -PathType Leaf)) {
            Fail "refusing to replace unmanaged plugin runtime: $Destination"
        }
        $Parent = Split-Path -Parent $Destination
        [IO.Directory]::CreateDirectory($Parent) | Out-Null
        $Stage = Join-Path $Parent (".install-$Target-" + [Guid]::NewGuid().ToString('N'))
        Copy-Item -LiteralPath $Source -Destination $Stage -Recurse
        Set-Content -LiteralPath (Join-Path $Stage ".kpopper-managed") -Value $Version -Encoding ASCII
        $Backup = Join-Path $Parent (".previous-$Target-" + [Guid]::NewGuid().ToString('N'))
        $HadPrevious = Test-Path -LiteralPath $Destination
        if ($HadPrevious) { Move-Item -LiteralPath $Destination -Destination $Backup }
        try {
            Move-Item -LiteralPath $Stage -Destination $Destination
        } catch {
            if (Test-Path -LiteralPath $Destination) { Remove-Item -LiteralPath $Destination -Recurse -Force }
            if ($HadPrevious -and (Test-Path -LiteralPath $Backup)) {
                Move-Item -LiteralPath $Backup -Destination $Destination
            }
            throw
        }
        if (Test-Path -LiteralPath $Backup) { Remove-Item -LiteralPath $Backup -Recurse -Force }
        Write-Output "Installed kpopper $Version plugin runtime at $Destination"
        return
    }

    if (-not $Prefix) { $Prefix = Join-Path $HOME ".local" }
    $PublicBin = Join-Path $Prefix "bin"
    $Destination = Join-Path $Prefix "lib/kpopper/$Version/$Target"
    $BinMarker = Join-Path $PublicBin ".kpopper-managed"
    $PublicResources = Join-Path $PublicBin "resources"
    foreach ($Name in @('kpop.exe', 'kpopper.exe')) {
        $Public = Join-Path $PublicBin $Name
        if ((Test-Path -LiteralPath $Public) -and -not (Test-Path -LiteralPath $BinMarker -PathType Leaf)) {
            Fail "refusing to replace unmanaged path: $Public"
        }
        if ((Test-Path -LiteralPath $Public) -and -not (Test-Path -LiteralPath $Public -PathType Leaf)) {
            Fail "refusing to replace non-file public path: $Public"
        }
    }
    if ((Test-Path -LiteralPath $PublicResources) -and -not (Test-Path -LiteralPath $BinMarker -PathType Leaf)) {
        Fail "refusing to replace unmanaged path: $PublicResources"
    }
    if ((Test-Path -LiteralPath $Destination) -and -not (Test-Path -LiteralPath (Join-Path $Destination ".kpopper-managed") -PathType Leaf)) {
        Fail "refusing to replace unmanaged version directory: $Destination"
    }
    if (Test-Path -LiteralPath $BinMarker -PathType Leaf) {
        $MarkerLines = @(Get-Content -LiteralPath $BinMarker)
        if ($MarkerLines.Count -ne 1 -or $MarkerLines[0] -notmatch '^([A-Za-z0-9._-]+)/windows-x86_64$' -or
            $Matches[1] -in @('.', '..')) {
            Fail "public ownership marker is invalid: $BinMarker"
        }
        $ManagedVersion = $Matches[1]
        $Previous = Join-Path $Prefix "lib/kpopper/$ManagedVersion/$Target"
        $PreviousMarker = Join-Path $Previous ".kpopper-managed"
        if (-not (Test-Path -LiteralPath $PreviousMarker -PathType Leaf) -or
            @((Get-Content -LiteralPath $PreviousMarker)).Count -ne 1 -or
            (Get-Content -LiteralPath $PreviousMarker -Raw).Trim() -ne $ManagedVersion) {
            Fail "public ownership marker does not resolve to a managed payload"
        }
        foreach ($Name in @('kpop.exe', 'kpopper.exe')) {
            $Public = Join-Path $PublicBin $Name
            if (Test-Path -LiteralPath $Public -PathType Leaf) {
                $Managed = Join-Path $Previous "bin/$Name"
                if (-not (Test-Path -LiteralPath $Managed -PathType Leaf) -or
                    (Get-FileHash -LiteralPath $Public -Algorithm SHA256).Hash -ne
                    (Get-FileHash -LiteralPath $Managed -Algorithm SHA256).Hash) {
                    Fail "refusing to replace modified public path: $Public"
                }
            }
        }
        if (Test-Path -LiteralPath $PublicResources) {
            if (-not (Test-Path -LiteralPath $PublicResources -PathType Container)) {
                Fail "refusing to replace non-directory public path: $PublicResources"
            }
            $ManagedResources = Join-Path $Previous "bin/resources"
            $Differences = @(Compare-Object -ReferenceObject @(Get-TreeInventory $ManagedResources) `
                                           -DifferenceObject @(Get-TreeInventory $PublicResources))
            if ($Differences.Count -ne 0) {
                Fail "refusing to replace modified public resource tree: $PublicResources"
            }
        }
    }
    [IO.Directory]::CreateDirectory((Split-Path -Parent $Destination)) | Out-Null
    [IO.Directory]::CreateDirectory($PublicBin) | Out-Null
    $Stage = Join-Path (Split-Path -Parent $Destination) (".install-$Target-" + [Guid]::NewGuid().ToString('N'))
    Copy-Item -LiteralPath $PackageRoot -Destination $Stage -Recurse
    Set-Content -LiteralPath (Join-Path $Stage ".kpopper-managed") -Value $Version -Encoding ASCII
    $Token = [Guid]::NewGuid().ToString('N')
    $DestinationBackup = Join-Path (Split-Path -Parent $Destination) ".previous-$Target-$Token"
    $HadDestination = Test-Path -LiteralPath $Destination
    $OldPublic = @{}
    foreach ($Name in @('kpop.exe', 'kpopper.exe')) {
        $Public = Join-Path $PublicBin $Name
        if (Test-Path -LiteralPath $Public -PathType Leaf) {
            $Saved = Join-Path $Work "old-$Name"
            Copy-Item -LiteralPath $Public -Destination $Saved
            $OldPublic[$Name] = $Saved
        }
    }
    $OldMarker = $null
    if (Test-Path -LiteralPath $BinMarker -PathType Leaf) {
        $OldMarker = Join-Path $Work "old-bin-marker"
        Copy-Item -LiteralPath $BinMarker -Destination $OldMarker
    }
    if ($HadDestination) { Move-Item -LiteralPath $Destination -Destination $DestinationBackup }
    $TemporaryResources = Join-Path $PublicBin ".install-resources-$Token"
    $ResourcesBackup = Join-Path $PublicBin ".previous-resources-$Token"
    $HadResources = Test-Path -LiteralPath $PublicResources
    $ResourcesDisplaced = $false
    $ResourcesActivated = $false
    try {
        Move-Item -LiteralPath $Stage -Destination $Destination
        Copy-Item -LiteralPath (Join-Path $Destination "bin/resources") -Destination $TemporaryResources -Recurse
        if ($HadResources) {
            Move-Item -LiteralPath $PublicResources -Destination $ResourcesBackup
            $ResourcesDisplaced = $true
        }
        Move-Item -LiteralPath $TemporaryResources -Destination $PublicResources
        $ResourcesActivated = $true
        foreach ($Name in @('kpop.exe', 'kpopper.exe')) {
            $Public = Join-Path $PublicBin $Name
            $TemporaryPublic = Join-Path $PublicBin ".install-$Name-$Token"
            Copy-Item -LiteralPath (Join-Path $Destination "bin/$Name") -Destination $TemporaryPublic
            Move-Item -LiteralPath $TemporaryPublic -Destination $Public -Force
        }
        $TemporaryMarker = Join-Path $PublicBin ".install-marker-$Token"
        Set-Content -LiteralPath $TemporaryMarker -Value "$Version/$Target" -Encoding ASCII
        Move-Item -LiteralPath $TemporaryMarker -Destination $BinMarker -Force
    } catch {
        foreach ($Name in @('kpop.exe', 'kpopper.exe')) {
            $Public = Join-Path $PublicBin $Name
            if ($OldPublic.ContainsKey($Name)) {
                Copy-Item -LiteralPath $OldPublic[$Name] -Destination $Public -Force
            } elseif (Test-Path -LiteralPath $Public) {
                Remove-Item -LiteralPath $Public -Force
            }
        }
        if ($OldMarker) {
            Copy-Item -LiteralPath $OldMarker -Destination $BinMarker -Force
        } elseif (Test-Path -LiteralPath $BinMarker) {
            Remove-Item -LiteralPath $BinMarker -Force
        }
        if ($ResourcesActivated -and (Test-Path -LiteralPath $PublicResources)) {
            Remove-Item -LiteralPath $PublicResources -Recurse -Force
        }
        if ($ResourcesDisplaced -and (Test-Path -LiteralPath $ResourcesBackup)) {
            Move-Item -LiteralPath $ResourcesBackup -Destination $PublicResources
        }
        foreach ($Temporary in @($TemporaryResources, (Join-Path $PublicBin ".install-kpop.exe-$Token"),
                                  (Join-Path $PublicBin ".install-kpopper.exe-$Token"),
                                  (Join-Path $PublicBin ".install-marker-$Token"))) {
            if (Test-Path -LiteralPath $Temporary) { Remove-Item -LiteralPath $Temporary -Recurse -Force }
        }
        if (Test-Path -LiteralPath $Destination) { Remove-Item -LiteralPath $Destination -Recurse -Force }
        if ($HadDestination -and (Test-Path -LiteralPath $DestinationBackup)) {
            Move-Item -LiteralPath $DestinationBackup -Destination $Destination
        }
        throw
    }
    if (Test-Path -LiteralPath $ResourcesBackup) { Remove-Item -LiteralPath $ResourcesBackup -Recurse -Force }
    if (Test-Path -LiteralPath $DestinationBackup) { Remove-Item -LiteralPath $DestinationBackup -Recurse -Force }
    Write-Output "Installed kpopper $Version at $Destination"
    Write-Output "Public commands: $(Join-Path $PublicBin 'kpop.exe') and $(Join-Path $PublicBin 'kpopper.exe')"
} finally {
    if (Test-Path -LiteralPath $Work) { Remove-Item -LiteralPath $Work -Recurse -Force }
}
