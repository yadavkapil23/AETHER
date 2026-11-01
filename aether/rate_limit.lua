local key = KEYS[1]
local now = tonumber(ARGV[1])
local window = tonumber(ARGV[2])
local limit = tonumber(ARGV[3])
local request_id = ARGV[4]

redis.call(
    "ZREMRANGEBYSCORE",
    key,
    0,
    now - window
)

local count = redis.call(
    "ZCARD",
    key
)

if count >= limit then
    local oldest = redis.call(
        "ZRANGE",
        key,
        0,
        0,
        "WITHSCORES"
    )

    local retry_after = window

    if oldest[2] then
        retry_after = math.ceil(
            window - (now - tonumber(oldest[2]))
        )
    end

    return {
        0,
        count,
        retry_after
    }
end

redis.call(
    "ZADD",
    key,
    now,
    request_id
)

redis.call(
    "EXPIRE",
    key,
    math.ceil(window)
)

return {
    1,
    count + 1,
    0
}