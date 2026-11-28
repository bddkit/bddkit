# The role of every step, and how a polled one went, in stream order. The
# number of attempts depends on timing, so only "more than one" is printed.
if .type == "step_started" then "\(.step) \(.role)"
elif .type == "step_finished" and .poll then "\(.step) \(.poll) \(.attempts > 1)"
else empty end
