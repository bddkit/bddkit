# The `attempt` of every evidence event that has one, in stream order.
select(.type == "evidence" and .attempt) | .attempt
