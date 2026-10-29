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
  cuando vence, muestra una notificación de Windows, trae la nota al frente y la
  marca como avisada. Los recordatorios que vencieron con la aplicación cerrada se
  avisan una sola vez al arrancar.
- **Listas de verificación**: cada nota es de texto o de lista. En modo lista cada
  elemento tiene casilla (tachado al marcarlo), `Enter` añade uno debajo, `Retroceso`
  en un elemento vacío lo borra y la `×` lo elimina. Al cambiar de modo, las líneas
  se convierten en elementos y viceversa (al volver a texto se pierde la marca de
  hecho/pendiente y se avisa antes).
- **Etiquetas**: texto libre por nota, sin duplicados (comparación sin distinguir
  mayúsculas).
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
- **Bandeja del sistema**: «Nueva nota», «Mostrar todas», «Ocultar todas», «Lista de
  notas», «Iniciar con Windows» y «Salir». El clic izquierdo en el icono muestra
  todas las notas. Cerrar una nota solo la oculta; la aplicación se cierra
  únicamente con «Salir».
- **Instancia única**: si se abre el ejecutable otra vez, solo se muestran las notas.

> ⚠️ **Si olvidas la contraseña de una nota, su contenido no se puede recuperar.**
> No hay puerta trasera ni copia del texto en claro: sin la contraseña, los datos
> cifrados son ilegibles, incluso para el desarrollador.

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

- `src-tauri/target/release/notas.exe` — ejecutable.
- `src-tauri/target/release/bundle/nsis/Notas_1.0.0_x64-setup.exe` — instalador NSIS
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

Contiene las notas, sus etiquetas y los recordatorios. Las notas con contraseña
guardan solo el texto cifrado; las columnas de texto en claro quedan vacías. La base
tiene `schema_version` (`PRAGMA user_version`) y se migra de forma idempotente al
arrancar. Para empezar de cero basta con cerrar la aplicación y borrar ese archivo.

«Iniciar con Windows» no se guarda en la base: es un valor llamado `Notas` bajo
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, que Windows consulta al iniciar
sesión. Al desactivarlo desde la bandeja, ese valor se elimina.

## Estructura

```
index.html            punto de entrada de las dos ventanas
src/
  main.ts             decide si la ventana es una nota o la lista
  note.ts             ventana de nota: cabecera, editor, lista, cerrojo
  list.ts             ventana «Lista de notas»
  api.ts              tipos y llamadas tipadas a los comandos de Rust
  ui.ts               ayudas de DOM, iconos, diálogos y avisos
  styles.css          hoja de estilos
src-tauri/
  src/db.rs           SQLite, migraciones y consultas
  src/crypto.rs       AES-256-GCM + Argon2id
  src/state.rs        conexión, claves de sesión e ítem de la bandeja
  src/windows.rs      creación, restauración y visibilidad de ventanas
  src/reminders.rs    hilo de recordatorios (cada 15 s)
  src/autostart.rs    «Iniciar con Windows» (primera ejecución y estado real)
  src/tray.rs         icono y menú de la bandeja
  src/commands.rs     comandos expuestos al frontend
  src/lib.rs          arranque, bandeja, instancia única, cierre
```

La interfaz nunca toca el disco ni las claves: todo pasa por comandos de Tauri.

## Licencia

MIT. Ver `LICENSE`.
