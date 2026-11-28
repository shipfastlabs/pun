use std::path::Path;

use pun_analyse::{Project, run};
use pun_phpstan::{Array, Key, Value as Parameter};
use serde_json::Value;

#[test]
fn larastan_app_extensions() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/larastan-app");
    let oracle: Value =
        serde_json::from_str(include_str!("fixtures/larastan-app/expected-phpstan.json")).unwrap();
    let mut parameters = Array::new();
    parameters.set(Key::from("level"), Parameter::Str("max".into()));
    parameters.set(Key::from("noModelMake"), Parameter::Bool(true));
    let mut project = Project::from_config(&root, &parameters);
    project.analysed = vec![
        root.join("app/Probe.php"),
        root.join("app/Console/Commands/ProbeCommand.php"),
    ];
    project.scanned = vec![
        root.join("laravel.php"),
        root.join("app/Models/User.php"),
        root.join("vendor/laravel/framework/src/Illuminate/Foundation/Application.php"),
    ];
    let prefix = format!("{}/", root.display());
    let mut got: Vec<_> = run(&project)
        .unwrap()
        .into_iter()
        .map(|e| {
            (
                e.file.strip_prefix(&prefix).unwrap_or(&e.file).to_string(),
                e.line,
                e.identifier,
                e.message,
            )
        })
        .collect();
    let mut want: Vec<_> = oracle
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["file"].as_str().unwrap_or("app/Probe.php").to_string(),
                e["line"].as_i64().unwrap(),
                e["identifier"].as_str().unwrap().to_string(),
                e["message"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    got.sort();
    want.sort();
    assert_eq!(got, want);
}

#[test]
fn bundled_stubs_without_larastan() {
    let root = std::env::temp_dir().join(format!("pun-bundled-stubs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let framework = root.join("vendor/laravel/framework/src/Illuminate/Foundation");
    std::fs::create_dir_all(&framework).unwrap();
    std::fs::create_dir_all(root.join("routes")).unwrap();
    std::fs::write(
        framework.join("Application.php"),
        "<?php\n\nnamespace Illuminate\\Foundation;\n\nclass Application\n{\n    const VERSION = '13.3.0';\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("laravel.php"),
        "<?php\n\nnamespace Illuminate\\Contracts\\Console {\n    interface Kernel {}\n}\n\nnamespace Illuminate\\Console {\n    class Command\n    {\n        public function comment(string $string): void {}\n    }\n}\n\nnamespace Illuminate\\Foundation\\Console {\n    class ClosureCommand extends \\Illuminate\\Console\\Command {}\n\n    class Kernel implements \\Illuminate\\Contracts\\Console\\Kernel\n    {\n        public function command($signature, \\Closure $callback)\n        {\n            return new ClosureCommand;\n        }\n    }\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("routes/console.php"),
        "<?php\n\n(new \\Illuminate\\Foundation\\Console\\Kernel)->command('inspire', function () {\n    $this->comment('hi');\n    $this->missing();\n});\n",
    )
    .unwrap();
    let mut parameters = Array::new();
    parameters.set(Key::from("level"), Parameter::Str("5".into()));
    parameters.set(Key::from("noModelMake"), Parameter::Bool(true));
    let mut project = Project::from_config(&root, &parameters);
    project.analysed = vec![root.join("routes/console.php")];
    project.scanned = vec![root.join("laravel.php"), framework.join("Application.php")];
    let got: Vec<_> = (run(&project).unwrap().into_iter())
        .map(|e| (e.line, e.identifier, e.message))
        .collect();
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(
        got,
        [(
            5,
            "method.notFound".to_string(),
            "Call to an undefined method Illuminate\\Foundation\\Console\\ClosureCommand::missing().".to_string()
        )]
    );
}

/// `'queue.connection' => [Queue::class]` makes the contract its own alias. A method the contract lacks used to
/// forward from the contract to itself until the stack overflowed; Larastan's container resolves a concrete queue
/// without the method, so PHPStan reports it as undefined.
#[test]
fn self_aliased_contract() {
    let root = std::env::temp_dir().join(format!("pun-self-alias-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let framework = root.join("vendor/laravel/framework/src/Illuminate/Foundation");
    std::fs::create_dir_all(&framework).unwrap();
    std::fs::create_dir_all(root.join("app")).unwrap();
    std::fs::write(
        framework.join("Application.php"),
        "<?php\n\nnamespace Illuminate\\Foundation;\n\nclass Application\n{\n    const VERSION = '13.3.0';\n\n    public function registerCoreContainerAliases()\n    {\n        foreach ([\n            'queue.connection' => [\\Illuminate\\Contracts\\Queue\\Queue::class],\n        ] as $key => $aliases) {\n            foreach ($aliases as $alias) {\n                $this->alias($key, $alias);\n            }\n        }\n    }\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("laravel.php"),
        "<?php\n\nnamespace Illuminate\\Contracts\\Queue;\n\ninterface Queue\n{\n    public function size($queue = null);\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("app/Probe.php"),
        "<?php\n\nfunction probe(\\Illuminate\\Contracts\\Queue\\Queue $queue): void\n{\n    $queue->size();\n    $queue->assertPushed('job');\n}\n",
    )
    .unwrap();
    let mut parameters = Array::new();
    parameters.set(Key::from("level"), Parameter::Str("max".into()));
    parameters.set(Key::from("noModelMake"), Parameter::Bool(true));
    let mut project = Project::from_config(&root, &parameters);
    project.analysed = vec![root.join("app/Probe.php")];
    project.scanned = vec![root.join("laravel.php"), framework.join("Application.php")];
    let got: Vec<_> = (run(&project).unwrap().into_iter())
        .map(|e| (e.line, e.identifier, e.message))
        .collect();
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(
        got,
        [(
            6,
            "method.notFound".to_string(),
            "Call to an undefined method Illuminate\\Contracts\\Queue\\Queue::assertPushed()."
                .to_string()
        )]
    );
}

/// Larastan asks the booted container for `app(ExceptionHandler::class)`: bootstrap/app.php's `withExceptions()` binds
/// the framework's handler, and a discovered package's provider (Collision's) binds its own over it. PHPStan 2.3.7
/// with Larastan 3.15.0 in a Laravel 13.44.1 app dumps `Illuminate\Foundation\Exceptions\Handler` without Collision
/// and `NunoMaduro\Collision\Adapters\Laravel\ExceptionHandler` with it.
#[test]
fn bootstrap_container_bindings() {
    let analyse = |package: bool| {
        let root = std::env::temp_dir().join(format!(
            "pun-bootstrap-bindings-{package}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let framework = root.join("vendor/laravel/framework/src/Illuminate/Foundation");
        std::fs::create_dir_all(&framework).unwrap();
        std::fs::create_dir_all(root.join("bootstrap")).unwrap();
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::write(
            framework.join("Application.php"),
            "<?php\n\nnamespace Illuminate\\Foundation;\n\nclass Application\n{\n    const VERSION = '13.44.1';\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("bootstrap/app.php"),
            "<?php\n\nreturn Application::configure(basePath: dirname(__DIR__))\n    ->withExceptions(function ($exceptions): void {\n    })->create();\n",
        )
        .unwrap();
        std::fs::write(
            root.join("laravel.php"),
            "<?php\n\nnamespace Illuminate\\Contracts\\Debug {\n    interface ExceptionHandler {}\n}\n\nnamespace Illuminate\\Foundation\\Exceptions {\n    class Handler implements \\Illuminate\\Contracts\\Debug\\ExceptionHandler\n    {\n        public function renderable(callable $renderUsing): static\n        {\n            return $this;\n        }\n    }\n}\n\nnamespace {\n    function app($abstract = null, array $parameters = []) {}\n}\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("vendor/composer")).unwrap();
        if package {
            let src = root.join("vendor/nunomaduro/collision/src/Adapters/Laravel");
            std::fs::create_dir_all(&src).unwrap();
            std::fs::write(
                root.join("vendor/composer/installed.json"),
                r#"{"packages": [{"name": "nunomaduro/collision", "install-path": "../nunomaduro/collision", "autoload": {"psr-4": {"NunoMaduro\\Collision\\": "src/"}}, "extra": {"laravel": {"providers": ["NunoMaduro\\Collision\\Adapters\\Laravel\\CollisionServiceProvider"]}}}]}"#,
            )
            .unwrap();
            std::fs::write(
                src.join("CollisionServiceProvider.php"),
                "<?php\n\nnamespace NunoMaduro\\Collision\\Adapters\\Laravel;\n\nuse Illuminate\\Contracts\\Debug\\ExceptionHandler as ExceptionHandlerContract;\n\nclass CollisionServiceProvider\n{\n    public function register(): void\n    {\n        $this->app->singleton(\n            ExceptionHandlerContract::class,\n            function ($app) {\n                return new ExceptionHandler($app);\n            }\n        );\n    }\n}\n",
            )
            .unwrap();
            std::fs::write(
                src.join("ExceptionHandler.php"),
                "<?php\n\nnamespace NunoMaduro\\Collision\\Adapters\\Laravel;\n\nfinal class ExceptionHandler implements \\Illuminate\\Contracts\\Debug\\ExceptionHandler\n{\n    public function renderable(callable $renderUsing): void {}\n}\n",
            )
            .unwrap();
        }
        std::fs::write(
            root.join("app/Probe.php"),
            "<?php\n\nuse Illuminate\\Contracts\\Debug\\ExceptionHandler;\n\nfunction probe(): void\n{\n    \\PHPStan\\dumpType(app(ExceptionHandler::class));\n    app(ExceptionHandler::class)->renderable(fn (): string => 'down');\n}\n",
        )
        .unwrap();
        let mut parameters = Array::new();
        parameters.set(Key::from("level"), Parameter::Str("6".into()));
        parameters.set(Key::from("noModelMake"), Parameter::Bool(true));
        parameters.set(
            Key::from("fileExtensions"),
            Parameter::Array(Array::list([Parameter::str("php")])),
        );
        let mut project = Project::from_config(&root, &parameters);
        project.analysed = vec![root.join("app/Probe.php")];
        project.scanned = vec![root.join("laravel.php"), framework.join("Application.php")];
        let got: Vec<_> = (run(&project).unwrap().into_iter())
            .map(|e| (e.line, e.identifier, e.message))
            .collect();
        std::fs::remove_dir_all(&root).unwrap();
        got
    };
    let dumped = |class: &str| {
        vec![(
            7,
            "phpstan.dumpType".to_string(),
            format!("Dumped type: {class}"),
        )]
    };
    assert_eq!(
        analyse(false),
        dumped("Illuminate\\Foundation\\Exceptions\\Handler")
    );
    assert_eq!(
        analyse(true),
        dumped("NunoMaduro\\Collision\\Adapters\\Laravel\\ExceptionHandler")
    );
}
