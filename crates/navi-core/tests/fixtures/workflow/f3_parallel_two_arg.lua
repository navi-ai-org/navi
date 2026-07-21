-- F3: negative — parallel(items, fn) must be rejected
function workflow()
  return parallel({"a", "b"}, function(x)
    return agent(x)
  end)
end
