<?php

namespace Rector\Tests\DeadCode\Rector\ClassMethod\RemoveEmptyClassMethodRector\Pun;

use Illuminate\Database\Migrations\Migration;

// Rector 2.9.2 removes down() and seed(), Rector 2.8.4 keeps them. Pun keeps an anonymous class's empty public
// and protected methods, as a Laravel migration's empty down() is the convention (shipfastlabs/pun#189).
return new class extends Migration
{
    public function up(): void
    {
        echo 'up';
    }

    public function down(): void {}

    protected function seed(): void {}

    private function unused(): void {}
};

?>
-----
<?php

namespace Rector\Tests\DeadCode\Rector\ClassMethod\RemoveEmptyClassMethodRector\Pun;

use Illuminate\Database\Migrations\Migration;

// Rector 2.9.2 removes down() and seed(), Rector 2.8.4 keeps them. Pun keeps an anonymous class's empty public
// and protected methods, as a Laravel migration's empty down() is the convention (shipfastlabs/pun#189).
return new class extends Migration
{
    public function up(): void
    {
        echo 'up';
    }

    public function down(): void {}

    protected function seed(): void {}
};

?>
