{
  studies_scanned: .nodes["m.astronomy_count"].computation.query_counts.input_count,
  astronomy_studies_selected: (.nodes["m.astronomy_count"].computation |
    if .status == "ok" then .query_counts.definite_match_count else .status // "unavailable" end),
  particle_identity_status: .nodes["m.particle_identities"].computation.status,
  review_basis: .nodes["d.review_scope"].state.basis.dependencies["m.astronomy_count"].basis_comparison
}
