# Every comment the stream carries, with the event that carries it.
select(.comment != null) | "\(.type): \(.comment)"
