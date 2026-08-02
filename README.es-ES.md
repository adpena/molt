

# Molt

Molt compila Python en binarios nativos independientes y WASM con un runtime de Rust, herramientas deterministas y límites de compatibilidad explícitos.

No intenta ser un lanzador oculto de CPython. Molt se enfoca en un subconjunto verificado y orientado a producción que puede seguir expandiéndose sin ceder el control sobre el rendimiento, el empaquetado o la semántica del runtime.

## Por qué Molt

- **Salida independiente**: los binarios compilados no dependen de una instalación de Python en el sistema host.
- **Runtime con prioridad en Rust**: la semántica de alto uso y el comportamiento de la stdlib se delegan a primitivas y funciones intrínsecas del runtime en lugar de recurrir a alternativas en Python.
- **Ingeniería determinista**: la paridad, el rendimiento y la seguridad se tratan como umbrales medibles, no como objetivos vagos.
- **Enfoque multiplataforma**: tanto los binarios nativos como WASM son objetivos de primera clase.

## Contrato del Proyecto

- Objetivo de paridad con CPython `>=3.12` para la semántica de Molt soportada.
- Objetivo del producto completo: paridad total con CPython `>=3.12` para el subconjunto soportado, sin dependencias ocultas del sistema host.
- Los artefactos compilados deben funcionar sin una instalación de Python en el sistema host.
- Por diseño, Molt no soporta `exec`/`eval`/`compile` sin restricciones, parcheo dinámico (monkeypatching) en tiempo de ejecución ni reflexión ilimitada en binarios compilados.

## Qué Soporta Molt Actualmente

- Compilación AOT nativa a través del backend de Rust.
- Flujos de trabajo para binarios independientes sin dependencias de runtime en CPython local.
- Un programa en expansión de lowering de la stdlib con prioridad en Rust, con superficies de auditoría generadas.
- Pruebas diferenciales contra CPython como vía principal de validación.
- Flujos de compilación para WASM, con la paridad entre plataformas aún incompleta y en seguimiento activo.

## Inicio Rápido en 5 Minutos

Para la configuración completa y la guía de solución de problemas, consulta [docs/getting-started.md](docs/getting-started.md).

```bash
uv sync --group dev --python 3.12   # installs the `molt` command into .venv
molt run examples/hello.py          # build + run, like `python examples/hello.py`
```

`uv sync` añade el comando `molt` a tu ruta de ejecución (en `.venv`). A partir de ahí, los comandos comunes son:

```bash
molt run app.py             # build and run (fast `dev` profile, like `cargo run`)
molt build app.py --release # produce an optimized standalone binary
./app                       # run the compiled binary directly
molt compare app.py         # diff Molt's output against CPython
```

> `molt run` utiliza por defecto el perfil rápido `dev` y `molt build` utiliza por defecto el perfil optimizado `release` — la misma convención que `cargo run` / `cargo build --release`. Puedes anular cualquiera de ellos con `--profile dev|release` (o el alias `--release`); ambos verbos aceptan ambos perfiles. Consulta [docs/getting-started.md](docs/getting-started.md#build-and-run-profiles).

> **Desde un clon del repositorio sin activar el entorno virtual**, anteponga cualquier comando con `uv run --python 3.12`, por ejemplo: `uv run --python 3.12 molt run examples/hello.py`. La forma de módulo `python3 -m molt.cli ...` es equivalente y es la que utilizan los flujos de verificación para colaboradores.

## Instalación

- Rutas de paquetes e instaladores: consulta [docs/getting-started.md](docs/getting-started.md)
- Detalles de empaquetado: [packaging/README.md](packaging/README.md)
- Comando de verificación: `molt doctor --json`

## Estado

El estado detallado actual se encuentra en [docs/spec/STATUS.md](docs/spec/STATUS.md). Las prioridades futuras están en [ROADMAP.md](ROADMAP.md). El plan de ejecución a corto plazo se detalla en [docs/ROADMAP_90_DAYS.md](docs/ROADMAP_90_DAYS.md).

Para detalles de compatibilidad y pruebas:

- Índice de documentación: [docs/INDEX.md](docs/INDEX.md)
- Índice de especificaciones: [docs/spec/README.md](docs/spec/README.md)
- Arquitectura de compatibilidad: [docs/spec/areas/compat/README.md](docs/spec/areas/compat/README.md)
- Informe detallado de benchmarks: [docs/benchmarks/bench_summary.md](docs/benchmarks/bench_summary.md)
- Flujo de trabajo de prueba independiente: [docs/proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md](docs/proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md)

## Desarrollo

- Mapa de colaboradores: [docs/DEVELOPER_GUIDE.md](docs/DEVELOPER_GUIDE.md)
- Operaciones y flujo de trabajo multi-agente: [docs/OPERATIONS.md](docs/OPERATIONS.md)
- Flujos de trabajo de benchmark: [docs/BENCHMARKING.md](docs/BENCHMARKING.md)
