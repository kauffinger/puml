<?php

namespace Shop\Orders;

enum OrderStatus: string
{
    case Pending = 'pending';
    case Paid = 'paid';
}
