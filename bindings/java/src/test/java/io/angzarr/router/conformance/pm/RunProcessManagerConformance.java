package io.angzarr.router.conformance.pm;

import static io.cucumber.junit.platform.engine.Constants.FEATURES_PROPERTY_NAME;
import static io.cucumber.junit.platform.engine.Constants.GLUE_PROPERTY_NAME;
import static io.cucumber.junit.platform.engine.Constants.PLUGIN_PROPERTY_NAME;

import org.junit.platform.suite.api.ConfigurationParameter;
import org.junit.platform.suite.api.IncludeEngines;
import org.junit.platform.suite.api.Suite;

/** Runs the shared process_manager.feature against the Java binding via Cucumber-JVM. */
@Suite
@IncludeEngines("cucumber")
@ConfigurationParameter(
    key = FEATURES_PROPERTY_NAME,
    value = "../../conformance/features/process_manager.feature")
@ConfigurationParameter(key = GLUE_PROPERTY_NAME, value = "io.angzarr.router.conformance.pm")
@ConfigurationParameter(key = PLUGIN_PROPERTY_NAME, value = "pretty")
public class RunProcessManagerConformance {}
