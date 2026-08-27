-- F1: enumerate → map → return (mock agents)
function workflow()
  phase("enumerate")
  local files = args.files or {}
  phase("map")
  local hits = pipeline(files, function(f)
    return agent("Audit " .. f, { label = f })
  end)
  return { count = #hits, hits = hits }
end
