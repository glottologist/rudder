/*
*************************************************************************************
* Copyright 2011 Normation SAS
*************************************************************************************
*
* Licensed under the Apache License, Version 2.0 (the "License");
* you may not use this file except in compliance with the License.
* You may obtain a copy of the License at
*
* http://www.apache.org/licenses/LICENSE-2.0
*
* Unless required by applicable law or agreed to in writing, software
* distributed under the License is distributed on an "AS IS" BASIS,
* WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
* See the License for the specific language governing permissions and
* limitations under the License.
*
*************************************************************************************
*/

package com.normation.eventlog

import org.joda.time.DateTime
import org.joda.time.format._
import scala.collection._
import scala.xml._
import java.security.Principal
import com.normation.utils.HashcodeCaching


final case class EventActor(name:String) extends HashcodeCaching

/**
 * A type that describe on what category an event belongs to. 
 */
trait EventLogCategory

private[eventlog] final case object UnknownLogCategory extends EventLogCategory

/**
 * Define the event log type, that will be serialized
 * the event class name minus "EventLog" is OK
 * It is a PartialFunction so the pattern matching are not a bottleneck anymore
 * (too much match ina  pattern matching usually fail)
 */
trait EventLogType extends PartialFunction[String, EventLogType] {
  def serialize : String
  
  override  def isDefinedAt(x : String) : Boolean = {
    serialize == x
  }
  
  def apply(x : String) = this
  
}

trait EventLogFilter extends PartialFunction[(EventLogType, EventLogDetails) , EventLog] {
  /**
   * An EventLogType used as identifier for that type of event.
   * Must be unique among all events. 
   * Most of the time, the event class name plus Type is OK. 
   */
  val eventType : EventLogType
  
  override  def isDefinedAt(x : (EventLogType, EventLogDetails)) : Boolean = {
    eventType == x._1
  }
  
  /**
   * This is used to simply build object from 
   */
  def apply(x : (EventLogType, EventLogDetails)) : EventLog 
  
}
/**
 * An EventLog is an object tracing activities on an entity.
 * It has an id (generated by the serialisation method), a type, a creation date,
 * a principal (the actor doing the action), a cause, a severity (like in syslog) and some raw data
 */
trait EventLog  {
  def eventDetails : EventLogDetails
  
  def id : Option[Int] = eventDetails.id // autogenerated id, by the serialization system

  //event log type is given by the implementation class. 
  //we only precise the category. 
  /**
   * Big category of the event
   */
  def eventLogCategory : EventLogCategory
  
  /**
   * An EventLogType used as identifier for that type of event.
   * Must be unique among all events. 
   * Most of the time, the event class name plus Type is OK. 
   */
  def eventType : EventLogType
  
  def principal : EventActor = eventDetails.principal
  
  def creationDate : DateTime = eventDetails.creationDate
  
  /**
   * When we create the EventLog, it usually shouldn't have an id, so the cause cannot be set
   * That why we have the EventLogTree that holds the hierarchy of EventLogs, the cause being used only when deserializing the object 
   */
  def cause : Option[Int] = eventDetails.cause 

  
  def severity : Int = eventDetails.severity
  
  /**
   * Some more (technical) details about the event, in a semi-structured
   * format (XML). 
   * 
   * Usually, the rawData will be computed from the fields when serializing, 
   * and be used to fill the fields when deserializing
   */
  def details : NodeSeq = eventDetails.details

  /**
   * Return a copy of the object with the cause set to given Id
   */
  def copySetCause(causeId:Int) : EventLog
}

/**
 * The unspecialized Event Log. Used as a container when unserializing data, to be specialized later by the EventLogSpecializers 
 */
case class UnspecializedEventLog(
    override val eventDetails : EventLogDetails
) extends EventLog with HashcodeCaching { 
  override val eventType = UnspecializedEventLog.eventType
  override val eventLogCategory = UnknownLogCategory
  override def copySetCause(causeId:Int) = this.copy(eventDetails.copy(cause = Some(causeId)))
  
}

object UnspecializedEventLog extends EventLogFilter {
  override val eventType = UnknownEventLogType
 
  override def apply(x : (EventLogType, EventLogDetails)) : UnspecializedEventLog = UnspecializedEventLog(x._2) 
}

object EventLog {  
  def withContent(nodes:NodeSeq) = <entry>{nodes}</entry>
  val emptyDetails = withContent(NodeSeq.Empty)
}

case object UnknownEventLogType extends EventLogType {
  def serialize = "UnknownType"   
}


/**
 * This case class holds all the important information 
 * about the EventLog 
 */
final case class EventLogDetails(
   val id : Option[Int] = None
 , val principal : EventActor
 , val creationDate : DateTime = DateTime.now()
 , val cause : Option[Int] = None
 , val severity : Int = 100
 , val reason  : Option[String]
 , val details : NodeSeq
) extends HashcodeCaching