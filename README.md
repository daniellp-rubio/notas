# Notas

Notas adhesivas para el escritorio de Windows. Cada nota es una ventana pequeña,
sin bordes y fuera de la barra de tareas, que conserva su contenido, color, tamaño,
posición y estado fijado entre reinicios.

## Qué hace

- **Notas de escritorio**: ventana propia por nota, sin bordes, redimensionable y
  arrastrable desde la barra superior. Se guardan la posición, el tamaño, el color
  y el contenido (con un retardo de ~0,5 s al escribir, mover o redimensionar) y se
  restauran al abrir la aplicación.
- **Colores**: amarillo (por defecto), rosa, verde, azul, morado y naranja, en tonos
  pastel con texto oscuro.
- **Siempre encima**: cada nota se puede fijar por encima del resto de ventanas.
- **Recordatorios**: fecha y hora local por nota. Un hilo en Rust revisa cada 15 s;
  cuando vence, muestra una notificación de Windows con el sonido normal, trae la
  nota al frente (la marca como visible, así sigue ahí tras reiniciar) y la deja
  como avisada. Los recordatorios que vencieron con la aplicación cerrada se avisan
  una sola vez al arrancar.
- **Notas de texto y listas de verificación**: cada nota es de texto enriquecido o de lista. El
  texto admite negrita, cursiva, subrayado, tachado, listas, tres tamaños y cinco colores desde
  «Aa»; el contenido se guarda como HTML saneado y se exporta a Markdown sin los formatos que
  Markdown no representa. En modo lista cada elemento tiene casilla (tachado al marcarlo),
  `Enter` añade uno debajo, `Retroceso` en un elemento vacío lo borra y la `×` lo elimina. Al
  cambiar de modo, las líneas se convierten en elementos y viceversa (al volver a texto se pierde
  la marca de hecho/pendiente y se avisa antes).
- **Etiquetas**: texto libre por nota, sin duplicados (comparación sin distinguir
  mayúsculas).
- **Pegar a una ventana**: desde el menú de una nota se elige una ventana abierta y
  se la pega a cualquier ventana de ese programa o a las que contengan un texto en
  el título. La nota solo aparece mientras la ventana coincida y esté al frente (o al
  editar la propia nota); «Despegar» la devuelve al escritorio. La regla se conserva
  en las copias JSON y la sincronización.
- **Lista de notas**: ventana normal, en la barra de tareas, con el color, el título
  (la primera línea del texto), las etiquetas y el estado de cada nota. Incluye
  búsqueda sobre las notas sin contraseña, filtro por etiqueta y acciones para
  mostrar, ocultar o eliminar. Las notas bloqueadas nunca muestran su contenido:
  aparecen como «🔒 Nota bloqueada».
- **Contraseña por nota**: el contenido se cifra en disco con **AES-256-GCM** y la
  clave se deriva con **Argon2id** (sal aleatoria de 16 bytes por nota, nonce
  aleatorio de 12 bytes por guardado). Mientras está bloqueada, la nota solo muestra
  la cabecera y un campo de contraseña; con la contraseña correcta se descifra en
  memoria y se puede editar, y cada guardado se vuelve a cifrar. La clave derivada
  solo vive en la memoria del proceso Rust y se descarta al volver a bloquear o al
  salir. Quitar la contraseña exige escribirla de nuevo.
- **Iniciar con Windows**: activo por defecto tras la primera ejecución. La
  aplicación se abre sola al iniciar sesión, como un arranque normal (restaura sus
  notas, deja el icono en la bandeja y pone en marcha los recordatorios). Se puede
  desactivar o volver a activar en cualquier momento desde la bandeja del sistema,
  en el ítem con marca de verificación **«Iniciar con Windows»**: lo que muestra la
  marca es el estado real de Windows. Una vez desactivado, seguirá desactivado en
  los siguientes arranques; la aplicación solo lo activa sola la primera vez.
- **Atajos de teclado globales**: `Ctrl+Alt+N` crea una nota (igual que «Nueva
  nota» de la bandeja) y `Ctrl+Alt+J` alterna entre ocultar todas las notas y
  mostrarlas todas. Funcionan con cualquier ventana activa. Si otro programa ya usa
  una combinación, la aplicación sigue funcionando y los ajustes muestran ese atajo
  como «No disponible (lo usa otro programa)».
- **Copias de seguridad automáticas**: al arrancar, si la última copia tiene más de
  24 h, y después cada 24 h mientras la aplicación está abierta, se guarda una copia
  de `notas.db` con la API de copia en línea de SQLite (consistente aunque haya
  escrituras en curso) en `backups\notas-AAAA-MM-DD.db`. Se conservan las 7 últimas.
  Las notas bloqueadas solo aparecen cifradas, porque la copia es del propio archivo.
- **Exportar e importar**: «Exportar a Markdown» escribe un `.md` con todas las
  notas sin contraseña (título como encabezado, cuerpo, listas como `- [ ]` / `- [x]`
  y una línea de etiquetas); las bloqueadas solo dejan el texto
  «🔒 Nota bloqueada (no se exporta su contenido)». «Exportar copia completa (.json)»
  guarda todo en un archivo versionado; las bloqueadas van como cifrado, sal y nonce
  en base64. También incluye reglas de «Pegar a una ventana». «Importar copia (.json)»
  añade las notas que faltan y solo reemplaza una nota existente cuando la copia es más
  nueva, con un resumen de cuántas se importaron, actualizaron o quedaron igual.
  «Restaurar copia automática» hace antes una copia de seguridad de las notas actuales
  y recarga las ventanas.
- **Ajustes**: ventana propia (en la barra de tareas) con «Iniciar con Windows», el
  estado de los atajos, las copias de seguridad (última fecha, hacer una copia ahora,
  restaurar, abrir la carpeta, exportar e importar) y sincronización por una carpeta
  elegida por ti. Nunca se activa sola.
- **Sincronización por carpeta**: guarda una nota JSON por archivo en
  `Notas-sync\notes\`, un `Notas.md` legible y `LEEME.md` dentro de una carpeta de
  OneDrive, Drive u otro servicio que elijas. Las notas bloqueadas permanecen
  cifradas; las notas desbloqueadas están legibles en `Notas.md`.
- **Bandeja del sistema**: «Nueva nota», «Mostrar todas», «Ocultar todas», «Lista de
  notas», «Ajustes…», «Iniciar con Windows» y «Salir». El clic izquierdo en el icono
  muestra todas las notas. Cerrar una nota solo la oculta; la aplicación se cierra
  únicamente con «Salir».
- **Instancia única**: si se abre el ejecutable otra vez, solo se muestran las notas.

> ⚠️ **Si olvidas la contraseña de una nota, su contenido no se puede recuperar.**
> No hay puerta trasera ni copia del texto en claro: sin la contraseña, los datos
> cifrados son ilegibles, incluso para el desarrollador.

> ⚠️ **Elige una carpeta privada para sincronizar.** La carpeta contiene los datos
> cifrados de las notas bloqueadas, pero también un `Notas.md` legible con todas las
> notas sin contraseña.

## Requisitos

Para usar la aplicación basta con Windows 10 u 11. Para compilarla desde el código:

- **Rust estable** instalado con [rustup](https://rustup.rs) y el toolchain
  `stable-x86_64-pc-windows-msvc` (es el predeterminado en Windows).
- **Microsoft C++ Build Tools**: instala *Visual Studio Build Tools* con la carga de
  trabajo **«Desarrollo para el escritorio con C++»** (el enlazador MSVC).
- **WebView2**: viene preinstalado en Windows 11 y en la mayoría de Windows 10. Si
  falta, instala el *Evergreen Runtime* de Microsoft.
- **Node.js 18 o superior** con npm.

## Compilar

Desde la raíz del repositorio:

```bash
npm install
npm run tauri build
```

El resultado queda en:

- `src-tauri/target/release/bundle/nsis/Notas_2.0.3_x64-setup.exe` — instalador NSIS
  para el usuario actual (no pide permisos de administrador).

Comprobaciones del proyecto:

```bash
cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cd ..
npx tsc --noEmit
npm run build
```

Para desarrollar con recarga en caliente: `npm run tauri dev`.

## Dónde viven los datos

Una única base SQLite en el directorio de datos de la aplicación:

```
%APPDATA%\com.dafel.notas\notas.db
```

Contiene las notas, sus etiquetas, recordatorios, reglas de ventana y el estado local
de sincronización. Las notas con contraseña guardan solo el texto cifrado; las columnas
de texto en claro quedan vacías. La base tiene `schema_version` (`PRAGMA user_version`)
y se migra de forma idempotente al arrancar. Para empezar de cero basta con cerrar la
aplicación y borrar ese archivo.

«Iniciar con Windows» no se guarda en la base: es un acceso directo `Notas.lnk` en la
carpeta Inicio del usuario (`%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup`),
que Windows abre al iniciar sesión. Al desactivarlo desde la bandeja, ese acceso
directo se borra. Las versiones anteriores a la 2.0.4 usaban un valor en
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`; Microsoft Defender lo marcaba
como sospechoso en un programa sin firma, así que al arrancar se elimina y se cambia
por el acceso directo.

### Aviso de Windows Defender o SmartScreen

El instalador y el programa no tienen firma digital. Por eso Windows puede mostrar
«Windows protegió tu PC» al instalar («Más información» → «Ejecutar de todas formas»).
El código es público en este repositorio para que cualquiera pueda revisarlo o
compilarlo.


## Cómo funciona la sincronización

Al elegir una carpeta, Notas crea `Notas-sync\`. Sincroniza al arrancar, cada 60
segundos, cinco segundos después de modificar una nota y al pulsar «Sincronizar ahora».
Cada archivo `notes\<id>.json` lleva `updated_at`, `device_id` y una marca de borrado.
Gana la versión más reciente; si empatan, gana el `device_id` mayor. Cuando ambas
versiones cambiaron desde la última sincronización correcta, también se guarda una copia
del perdedor con el título «(conflicto) …». Los borrados se propagan como marcas que se
conservan 30 días. Una posición, tamaño o visibilidad nunca se copia entre dispositivos.
Los archivos JSON corruptos se omiten y el error aparece en Ajustes.
## Estructura

```
index.html            punto de entrada de las dos ventanas
src/
  main.ts             decide si la ventana es una nota, la lista o los ajustes
  note.ts             ventana de nota: cabecera, editor, lista, cerrojo
  list.ts             ventana «Lista de notas»
  settings.ts         ventana «Ajustes»
  api.ts              tipos y llamadas tipadas a los comandos de Rust
  ui.ts               ayudas de DOM, iconos, diálogos y avisos
  styles.css          hoja de estilos
src-tauri/
  src/db.rs           SQLite, migraciones y consultas
  src/crypto.rs       AES-256-GCM + Argon2id
  src/state.rs        conexión, claves de sesión e ítem de la bandeja
  src/windows.rs      creación, restauración y visibilidad de ventanas
  src/attachments.rs  reglas de ventana en primer plano
  src/sync.rs         sincronización de carpeta y resolución de conflictos
  src/reminders.rs    hilo de recordatorios (cada 15 s)
  src/backup.rs       copias automáticas, exportar e importar
  src/shortcuts.rs    atajos de teclado globales
  src/autostart.rs    «Iniciar con Windows» (primera ejecución y estado real)
  src/tray.rs         icono y menú de la bandeja
  src/commands.rs     comandos expuestos al frontend
  src/lib.rs          arranque, bandeja, instancia única, cierre
```

La interfaz nunca toca el disco ni las claves: todo pasa por comandos de Tauri.

## Licencia

MIT. Ver `LICENSE`.
