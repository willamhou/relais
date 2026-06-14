param(
    [string]$BaseUrl = "https://api.tffair.cn/1",
    [string]$LoginName,
    [string]$Password,
    [string]$Token,
    [string]$CustomerId,
    [Parameter(Mandatory = $true)]
    [string]$GoodsKeyword,
    [Parameter(Mandatory = $true)]
    [int]$Amount,
    [Parameter(Mandatory = $true)]
    [string]$Receiver,
    [switch]$FullLogin,
    [switch]$SubmitOrder
)

$ErrorActionPreference = "Stop"

function Invoke-JsonPost {
    param(
        [Parameter(Mandatory = $true)][string]$Url,
        [Parameter(Mandatory = $true)]$Body,
        [int]$TimeoutSec = 60
    )

    $json = $Body | ConvertTo-Json -Depth 12 -Compress
    $response = Invoke-RestMethod -Uri $Url -Method Post -ContentType "application/json" -Body $json -TimeoutSec $TimeoutSec
    if ($response.err_code) {
        throw "$Url failed: $($response.err_code) $($response.err_msg)"
    }
    return $response
}

function New-OrderNo {
    return (Get-Date -Format "yyMMddHHmmss") + (Get-Random -Minimum 10000000 -Maximum 99999999)
}

if ($FullLogin -or -not $Token) {
    if (-not $LoginName -or -not $Password) {
        throw "LoginName and Password are required when FullLogin is set or Token is omitted."
    }

    $login = Invoke-JsonPost -Url "$BaseUrl/login/do" -Body @{
        login_name = $LoginName
        password   = $Password
    } -TimeoutSec 30

    $Token = $login.acs_token
    $CustomerId = $login.customer_id
    if (-not $Token -or -not $CustomerId) {
        throw "Login response missing acs_token or customer_id."
    }
} elseif (-not $CustomerId) {
    throw "CustomerId is required when using Token directly."
}

$goodsResult = Invoke-JsonPost -Url "$BaseUrl/edi_api/goods/website_goods_to" -Body @{
    acs_token         = $Token
    service_object_id = $CustomerId
    page_num          = 1
    page_size         = 20
    keyword           = $GoodsKeyword
} -TimeoutSec 30

$goods = @($goodsResult.data_list) | Where-Object { $_.name -eq $GoodsKeyword } | Select-Object -First 1
if (-not $goods) {
    $goods = @($goodsResult.data_list) | Where-Object { $_.name -like "*$GoodsKeyword*" } | Select-Object -First 1
}
if (-not $goods) {
    throw "Goods not found: $GoodsKeyword"
}

$createCart = Invoke-JsonPost -Url "$BaseUrl/shoppingcarts/create" -Body ([ordered]@{
    acs_token             = $Token
    service_object_id     = $CustomerId
    goods_id              = $goods.goods_id
    supplier_id           = $goods.supplier_id
    unit_id               = $goods.unit_id
    rating_form_detail_id = $goods.rating_form_detail_id
    flat_price_id         = $goods.flat_price_id
    agent_id              = $goods.agent_id
    agent_price_id        = $goods.agent_price_id
    agent_cust_price_id   = $goods.agent_cust_price_id
    supplier_alias_id     = $goods.supplier_alias_id
    order_amount          = $Amount
    is_recycle_bottle     = 0
}) -TimeoutSec 30

$newCartId = $createCart.id

$addressResult = Invoke-JsonPost -Url "$BaseUrl/customers/customers_receive_list" -Body @{
    acs_token    = $Token
    customer_id  = $CustomerId
    page_num     = 1
    page_size    = 50
    receive_type = 0
} -TimeoutSec 30

# Receiver matching is intentionally exact on contact_info. Do not match customer_name.
$address = @($addressResult.data_list) | Where-Object { $_.contact_info -eq $Receiver } | Select-Object -First 1
if (-not $address) {
    throw "Receiver not found by exact contact_info match: $Receiver"
}

$receiveId = $address.receive_id
if (-not $receiveId) {
    $receiveId = $address.id
}

$cartQueryBody = @{
    acs_token          = $Token
    customer_id        = $CustomerId
    is_recycle_bottle  = 0
    receive_address_id = $receiveId
}

$cart = Invoke-JsonPost -Url "$BaseUrl/shoppingcarts/getShoppingCarts" -Body $cartQueryBody -TimeoutSec 30
$cartItems = @($cart.cart_items)
$targetCart = $cartItems | Where-Object { $_.cart_id -eq $newCartId } | Select-Object -First 1
if (-not $targetCart) {
    $targetCart = $cartItems | Where-Object { $_.goods_id -eq $goods.goods_id } | Sort-Object cart_id -Descending | Select-Object -First 1
}
if (-not $targetCart) {
    throw "Target cart item not found after add."
}

if ([int]$targetCart.amount -ne $Amount) {
    Invoke-JsonPost -Url "$BaseUrl/shoppingcarts/update" -Body @{
        acs_token         = $Token
        service_object_id = $CustomerId
        id                = $targetCart.cart_id
        amount            = $Amount
        supplier_id       = $targetCart.supplier_id
        goods_id          = $targetCart.goods_id
        goods_count_once  = $targetCart.goods_count_to_shopping_cart
        order_memo        = ""
    } -TimeoutSec 30 | Out-Null
}

$cart = Invoke-JsonPost -Url "$BaseUrl/shoppingcarts/getShoppingCarts" -Body $cartQueryBody -TimeoutSec 30
$cartItems = @($cart.cart_items)
$targetCart = $cartItems | Where-Object { $_.cart_id -eq $targetCart.cart_id } | Select-Object -First 1
if (-not $targetCart) {
    throw "Target cart item not found after quantity update."
}
if ([int]$targetCart.amount -ne $Amount) {
    throw "Target cart amount is $($targetCart.amount), expected $Amount."
}

$selectList = @()
foreach ($cartItem in $cartItems) {
    $status = 2
    if ($cartItem.cart_id -eq $targetCart.cart_id) {
        $status = 1
    }
    $selectList += @{
        id            = $cartItem.cart_id
        select_status = $status
    }
}

Invoke-JsonPost -Url "$BaseUrl/shoppingcarts/select" -Body @{
    acs_token         = $Token
    service_object_id = $CustomerId
    select            = $selectList
} -TimeoutSec 30 | Out-Null

$cart = Invoke-JsonPost -Url "$BaseUrl/shoppingcarts/getShoppingCarts" -Body $cartQueryBody -TimeoutSec 30
$selected = @(@($cart.cart_items) | Where-Object { $_.select_status -eq 1 })
if ($selected.Count -ne 1 -or $selected[0].cart_id -ne $targetCart.cart_id) {
    $selectedIds = ($selected | ForEach-Object { $_.cart_id }) -join ","
    throw "Unexpected selected cart items: $selectedIds"
}

$result = [ordered]@{
    order_called = $false
    goods        = [ordered]@{
        cart_id   = $targetCart.cart_id
        goods_id  = $targetCart.goods_id
        name      = $targetCart.goods_name
        amount    = $targetCart.amount
        total_amt = $cart.total_amt
    }
    address      = [ordered]@{
        receive_id    = $receiveId
        customer_id   = $address.customer_id
        customer_name = $address.customer_name
        contact_info  = $address.contact_info
        type          = $address.type
        status        = $address.status
    }
    selected_before_order = @($selected | ForEach-Object { $_.cart_id })
}

if ($SubmitOrder) {
    $activityIds = @()
    if ($selected[0].activity_info) {
        $activityIds = @($selected[0].activity_info | ForEach-Object { $_.activity_id })
    }

    $orderNo = New-OrderNo
    $order = Invoke-JsonPost -Url "$BaseUrl/orders/order_for_all" -Body ([ordered]@{
        acs_token              = $Token
        order_no               = $orderNo
        address_item           = @(@{
            agent_id        = ""
            customer_id     = $CustomerId
            receive_info_id = $receiveId
        })
        shopping_cart_ids      = @($selected[0].cart_id)
        total_amt              = $cart.total_amt
        shopping_carts         = @(@{
            id                     = $selected[0].cart_id
            activity_customer_type = 0
            activity_ids           = $activityIds
        })
        service_object_id      = $CustomerId
        recycle_bottle_voucher = @()
    }) -TimeoutSec 60

    $result.order_called = $true
    $result.request_order_no = $orderNo
    $result.order_response = $order
}

$result | ConvertTo-Json -Depth 12
