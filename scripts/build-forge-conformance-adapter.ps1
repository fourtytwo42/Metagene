param(
    [Parameter(Mandatory = $true)]
    [string] $ForgeJar
)

$ErrorActionPreference = 'Stop'
$repository = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$source = Join-Path $repository 'src-tauri\resources\forge-adapter\ForgeConformanceAdapter.java'
$output = Join-Path $repository 'src-tauri\resources\forge-adapter\forge-conformance-adapter.jar'
$temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$build = Join-Path $temporaryRoot ('metagene-forge-adapter-' + [guid]::NewGuid().ToString('N'))

if (-not (Test-Path -LiteralPath $ForgeJar -PathType Leaf)) {
    throw "Forge runtime jar does not exist: $ForgeJar"
}

try {
    New-Item -ItemType Directory -Path $build | Out-Null
    $compiler = Join-Path $build 'ecj.jar'
    Invoke-WebRequest -UseBasicParsing `
        'https://repo1.maven.org/maven2/org/eclipse/jdt/ecj/3.38.0/ecj-3.38.0.jar' `
        -OutFile $compiler
    & java -jar $compiler -17 -cp $ForgeJar -d $build $source
    if ($LASTEXITCODE -ne 0) {
        throw "ECJ failed with exit code $LASTEXITCODE"
    }

    if (Test-Path -LiteralPath $output) {
        [IO.File]::Delete($output)
    }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [IO.Compression.ZipFile]::Open($output, [IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($classFile in Get-ChildItem -LiteralPath $build -Filter 'ForgeConformanceAdapter*.class') {
            [IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
                $archive,
                $classFile.FullName,
                $classFile.Name,
                [IO.Compression.CompressionLevel]::Optimal
            ) | Out-Null
        }
    }
    finally {
        $archive.Dispose()
    }
    Get-FileHash -Algorithm SHA256 $output
}
finally {
    $resolvedBuild = [IO.Path]::GetFullPath($build)
    if ($resolvedBuild.StartsWith($temporaryRoot, [StringComparison]::OrdinalIgnoreCase) -and
        (Test-Path -LiteralPath $resolvedBuild)) {
        [IO.Directory]::Delete($resolvedBuild, $true)
    }
}
