# Integración de Orion con PowerShell.
#
# Marca dónde empieza y termina cada comando con secuencias OSC 133 (un estándar de las terminales).
# Orion las lee para armar los bloques:
#
#   ESC]133;A BEL   empieza el prompt
#   ESC]133;B BEL   termina el prompt: lo que sigue es el comando que escribes
#   ESC]133;C BEL   apretaste Enter: lo que sigue es la salida del comando
#   ESC]133;D;n BEL terminó el comando, con código de salida n
#
# Además pone el título (OSC 0) con el nombre de la carpeta actual: es lo que muestra la pestaña.
#
# Se carga después de tu perfil, así que envuelve el prompt que ya tengas, aunque sea uno personalizado.

$Global:__OrionPrompt = $function:prompt
$Global:__OrionLastId = -1  # -1 = todavía no se mostró ningún prompt

function Global:prompt {
    # $? tiene que ser lo primero que se lee: cualquier otra línea lo sobrescribe.
    $ok = $?
    $code = if ($ok) { 0 } elseif ($LASTEXITCODE) { $LASTEXITCODE } else { 1 }
    $e = [char]27
    $bel = [char]7
    $marks = ''

    $last = Get-History -Count 1
    $lastId = if ($last) { $last.Id } else { 0 }
    if ($Global:__OrionLastId -ne -1) {
        # Si el historial no avanzó, apretaste Enter sin escribir nada: el bloque termina sin código.
        if ($lastId -ne $Global:__OrionLastId) { $marks += "$e]133;D;$code$bel" }
        else { $marks += "$e]133;D$bel" }
    }
    $Global:__OrionLastId = $lastId

    # Título de la pestaña (OSC 0): el nombre de la carpeta actual (en tu carpeta de usuario, tu usuario).
    $dir = $PWD.ProviderPath
    $title = Split-Path -Leaf $dir
    if (-not $title) { $title = $dir }  # en la raíz (C:\) no hay "último pedazo"
    $marks += "$e]0;$title$bel"

    $marks += "$e]133;A$bel"
    $marks += & $Global:__OrionPrompt
    $marks + "$e]133;B$bel"
}

# Autosugerencias: mientras escribes aparece en gris un comando parecido del historial.
# → (o Fin) lo acepta entero, Ctrl+→ palabra por palabra, F2 cambia a una lista con varias opciones.
# Las hace PSReadLine 2.1+ (viene con PowerShell 7). Si en tu perfil las apagaste, se respeta.
$readLine = Get-Module PSReadLine
if ($readLine -and $readLine.Version -ge [version]'2.1' -and (Get-PSReadLineOption).PredictionSource -eq 'None') {
    # HistoryAndPlugin suma sugerencias de módulos (ej. Az, CompletionPredictor); necesita PowerShell 7.2+.
    try { Set-PSReadLineOption -PredictionSource HistoryAndPlugin }
    catch { Set-PSReadLineOption -PredictionSource History }
}
if ($readLine -and $readLine.Version -ge [version]'2.1') {
    # Gris del tema de Orion (ANSI 90) en vez del blanco tenue de PSReadLine.
    Set-PSReadLineOption -Colors @{ InlinePrediction = "$([char]27)[90m" }
}

# PSReadLine lee lo que escribes con esta función. La envolvemos para marcar el momento del Enter.
if (Test-Path Function:\PSConsoleHostReadLine) {
    $Global:__OrionReadLine = $function:PSConsoleHostReadLine
    function Global:PSConsoleHostReadLine {
        $line = & $Global:__OrionReadLine
        [Console]::Write("$([char]27)]133;C$([char]7)")
        $line
    }
}
