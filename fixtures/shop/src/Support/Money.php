<?php

namespace Shop\Support;

final class Money
{
    public function __construct(public readonly int $cents)
    {
    }

    public static function of(int $cents): self
    {
        return new self($cents);
    }
}
