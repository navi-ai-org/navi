-- F2: parallel thunks + write_allow pattern (GLM-style)
function workflow()
  phase("parallel_review")
  local results = parallel({
    function()
      return agent("Review src/a.rs", {
        label = "a",
        write_allow = {"src/a.rs"},
        tools = {"read_file", "edit"},
      })
    end,
    function()
      return agent("Review README", {
        label = "readme",
        tools = {"read_file", "search"},
      })
    end,
  })
  return { n = #results, results = results }
end
